use crate::{
    sim::SimChunk,
    site::Site,
    util::{map_array::GenericIndex, DHashMap, DHashSet},
};
use common::{
    store::Id,
    terrain::BiomeKind,
    trade::{Good, SitePrices},
};
use lazy_static::lazy_static;
use std::{cmp::Ordering::Less, convert::TryFrom};
use tracing::{debug, info};

use Good::*;
mod map_types;
pub use map_types::{GoodIndex, GoodMap, Labor, LaborIndex, LaborMap, NaturalResources};

pub const INTER_SITE_TRADE: bool = true;
pub const DAYS_PER_MONTH: f32 = 30.0;
pub const DAYS_PER_YEAR: f32 = 12.0 * DAYS_PER_MONTH;

// this is an empty replacement for https://github.com/cpetig/vergleich
// which can be used to compare values acros runs
pub mod vergleich {
    pub struct Error {}
    impl Error {
        pub fn to_string(&self) -> &'static str { "" }
    }
    pub struct ProgramRun {}
    impl ProgramRun {
        pub fn new(_: &str) -> Result<Self, Error> { Ok(Self {}) }

        pub fn set_epsilon(&mut self, _: f32) {}

        pub fn context(&mut self, _: &str) -> Context { Context {} }

        //pub fn value(&mut self, _: &str, val: f32) -> f32 { val }
    }
    pub struct Context {}
    impl Context {
        #[must_use]
        pub fn context(&mut self, _: &str) -> Context { Context {} }

        pub fn value(&mut self, _: &str, val: f32) -> f32 { val }

        pub fn dummy() -> Self { Context {} }
    }
}

#[derive(Debug)]
pub struct TradeOrder {
    pub customer: Id<Site>,
    pub amount: GoodMap<f32>, // positive for orders, negative for exchange
}

#[derive(Debug)]
pub struct TradeDelivery {
    pub supplier: Id<Site>,
    pub amount: GoodMap<f32>, // positive for orders, negative for exchange
    pub prices: GoodMap<f32>, // at the time of interaction
    pub supply: GoodMap<f32>, // maximum amount available, at the time of interaction
}

#[derive(Debug, Default)]
pub struct TradeInformation {
    pub orders: DHashMap<Id<Site>, Vec<TradeOrder>>, // per provider
    pub deliveries: DHashMap<Id<Site>, Vec<TradeDelivery>>, // per receiver
}

#[derive(Debug)]
pub struct NeighborInformation {
    pub id: Id<Site>,
    pub travel_distance: usize,

    // remembered from last interaction
    pub last_values: GoodMap<f32>,
    pub last_supplies: GoodMap<f32>,
}

lazy_static! {
    pub static ref COIN_INDEX: GoodIndex = Coin.try_into().unwrap_or_default();
    static ref FOOD_INDEX: GoodIndex = Good::Food.try_into().unwrap_or_default();
    static ref TRANSPORTATION_INDEX: GoodIndex = Transportation.try_into().unwrap_or_default();
}

#[derive(Debug)]
pub struct Economy {
    /// Population
    pub pop: f32,
    pub population_limited_by: GoodIndex,

    /// Total available amount of each good
    pub stocks: GoodMap<f32>,
    /// Surplus stock compared to demand orders
    pub surplus: GoodMap<f32>,
    /// change rate (derivative) of stock in the current situation
    pub marginal_surplus: GoodMap<f32>,
    /// amount of wares not needed by the economy (helps with trade planning)
    pub unconsumed_stock: GoodMap<f32>,
    /// Local availability of a good, 4.0 = starved, 2.0 = balanced, 0.1 =
    /// extra, NULL = way too much
    // For some goods, such a goods without any supply, it doesn't make sense to talk about value
    pub values: GoodMap<Option<f32>>,
    /// amount of goods exported/imported during the last cycle
    pub last_exports: GoodMap<f32>,
    pub active_exports: GoodMap<f32>, // unfinished trade (amount unconfirmed)
    //pub export_targets: GoodMap<f32>,
    /// amount of labor that went into a good, [1 man cycle=1.0]
    pub labor_values: GoodMap<Option<f32>>,
    // this assumes a single source, replace with LaborMap?
    pub material_costs: GoodMap<f32>,

    /// Proportion of individuals dedicated to an industry (sums to roughly 1.0)
    pub labors: LaborMap<f32>,
    // Per worker, per year, of their output good
    pub yields: LaborMap<f32>,
    /// [0.0..1.0]
    pub productivity: LaborMap<f32>,
    /// Missing raw material which limits production
    pub limited_by: LaborMap<GoodIndex>,

    pub natural_resources: NaturalResources,
    /// Neighboring sites to trade with
    pub neighbors: Vec<NeighborInformation>,

    /// outgoing trade, per provider
    pub orders: DHashMap<Id<Site>, Vec<TradeOrder>>,
    /// incoming trade - only towards this site
    pub deliveries: Vec<TradeDelivery>,
}

impl Default for Economy {
    fn default() -> Self {
        let coin_index: GoodIndex = GoodIndex::try_from(Coin).unwrap_or_default();
        Self {
            pop: 32.0,
            population_limited_by: GoodIndex::default(),

            stocks: GoodMap::from_list(&[(coin_index, Economy::STARTING_COIN)], 100.0),
            surplus: Default::default(),
            marginal_surplus: Default::default(),
            values: GoodMap::from_list(&[(coin_index, Some(2.0))], None),
            last_exports: Default::default(),
            active_exports: Default::default(),

            labor_values: Default::default(),
            material_costs: Default::default(),

            labors: LaborMap::from_default(0.01),
            yields: LaborMap::from_default(1.0),
            productivity: LaborMap::from_default(1.0),
            limited_by: LaborMap::from_default(GoodIndex::default()),

            natural_resources: Default::default(),
            neighbors: Default::default(),
            unconsumed_stock: Default::default(),

            orders: Default::default(),
            deliveries: Default::default(),
        }
    }
}

impl Economy {
    pub const MINIMUM_PRICE: f32 = 0.1;
    pub const STARTING_COIN: f32 = 1000.0;
    const _NATURAL_RESOURCE_SCALE: f32 = 1.0 / 9.0;

    pub fn cache_economy(&mut self) {
        for g in good_list() {
            let amount: f32 = self
                .natural_resources
                .per_area
                .iter()
                .map(|a| a.resource_sum[g])
                .sum();
            let chunks = self
                .natural_resources
                .per_area
                .iter()
                .map(|a| a.resource_chunks[g])
                .sum();
            if chunks > 0.001 {
                self.natural_resources.chunks_per_resource[g] = chunks;
                self.natural_resources.average_yield_per_chunk[g] = amount / chunks;
            }
        }
    }

    pub fn get_orders(&self) -> DHashMap<Option<LaborIndex>, Vec<(GoodIndex, f32)>> {
        Labor::list_full()
            .map(|l| (if l.is_everyone() { None } else { Some(l) }, l.orders()))
            .collect()
    }

    pub fn get_productivity(&self) -> LaborMap<(GoodIndex, f32)> {
        // cache the site independent part of production
        lazy_static! {
            static ref PRODUCTS: LaborMap<(GoodIndex, f32)> = LaborMap::from_iter(
                Labor::list().map(|p| { (p, p.products(),) }),
                (GoodIndex::default(), 0.0),
            );
        }
        PRODUCTS.map(|l, vec| {
            //dbg!((l,vec));
            let labor_ratio = self.labors[l];
            let total_workers = labor_ratio * self.pop;
            // apply economy of scale (workers get more productive in numbers)
            let relative_scale = 1.0 + labor_ratio;
            let absolute_scale = (1.0 + total_workers / 100.0).min(3.0);
            let scale = relative_scale * absolute_scale;
            (vec.0, vec.1 * scale)
        })
    }

    pub fn replenish(&mut self, _time: f32) {
        for (good, &ch) in self.natural_resources.chunks_per_resource.iter() {
            let per_year = self.natural_resources.average_yield_per_chunk[good] * ch;
            self.stocks[good] = self.stocks[good].max(per_year);
        }
        // info!("resources {:?}", self.stocks);
    }

    pub fn add_chunk(&mut self, ch: &SimChunk, distance_squared: i64) {
        // let biome = ch.get_biome();
        // we don't scale by pi, although that would be correct
        let distance_bin = (distance_squared >> 16).min(64) as usize;
        if self.natural_resources.per_area.len() <= distance_bin {
            self.natural_resources
                .per_area
                .resize_with(distance_bin + 1, Default::default);
        }
        self.natural_resources.per_area[distance_bin].chunks += 1;

        let mut add_biome = |biome, amount| {
            if let Ok(idx) = GoodIndex::try_from(Terrain(biome)) {
                self.natural_resources.per_area[distance_bin].resource_sum[idx] += amount;
                self.natural_resources.per_area[distance_bin].resource_chunks[idx] += amount;
            }
        };
        if ch.river.is_ocean() {
            add_biome(BiomeKind::Ocean, 1.0);
        } else if ch.river.is_lake() {
            add_biome(BiomeKind::Lake, 1.0);
        } else {
            add_biome(BiomeKind::Forest, 0.5 + ch.tree_density);
            add_biome(BiomeKind::Grassland, 0.5 + ch.humidity);
            add_biome(BiomeKind::Jungle, 0.5 + ch.humidity * ch.temp.max(0.0));
            add_biome(BiomeKind::Mountain, 0.5 + (ch.alt / 4000.0).max(0.0));
            add_biome(
                BiomeKind::Desert,
                0.5 + (1.0 - ch.humidity) * ch.temp.max(0.0),
            );
            add_biome(BiomeKind::Snowland, 0.5 + (-ch.temp).max(0.0));
        }
    }

    pub fn add_neighbor(&mut self, id: Id<Site>, distance: usize) {
        self.neighbors.push(NeighborInformation {
            id,
            travel_distance: distance,

            last_values: GoodMap::from_default(Economy::MINIMUM_PRICE),
            last_supplies: Default::default(),
        });
    }

    pub fn get_site_prices(&self) -> SitePrices {
        let normalize = |xs: GoodMap<Option<f32>>| {
            let sum = xs
                .iter()
                .map(|(_, x)| (*x).unwrap_or(0.0))
                .sum::<f32>()
                .max(0.001);
            xs.map(|_, x| Some(x? / sum))
        };

        SitePrices {
            values: {
                let labor_values = normalize(self.labor_values);
                // Use labor values as prices. Not correct (doesn't care about exchange value)
                let prices = normalize(self.values).map(|good, value| {
                    ((labor_values[good].unwrap_or(Economy::MINIMUM_PRICE)
                        + value.unwrap_or(Economy::MINIMUM_PRICE))
                        * 0.5)
                        .max(Economy::MINIMUM_PRICE)
                });
                prices.iter().map(|(g, v)| (Good::from(g), *v)).collect()
            },
        }
    }

    /// plan the trading according to missing goods and prices at neighboring
    /// sites (1st step of trading)
    // returns wares spent (-) and procured (+)
    // potential_trade: positive = buy, (negative = sell, unused)
    fn plan_trade_for_site(
        // site: &mut Site,
        &mut self,
        site_id: &Id<Site>,
        transportation_capacity: f32,
        // external_orders: &mut DHashMap<Id<Site>, Vec<TradeOrder>>,
        potential_trade: &mut GoodMap<f32>,
    ) -> GoodMap<f32> {
        // TODO: Do we have some latency of information here (using last years
        // capacity?)
        //let total_transport_capacity = self.stocks[Transportation];
        // TODO: We don't count the capacity per site, but globally (so there might be
        // some imbalance in dispatch vs collection across sites (e.g. more dispatch
        // than collection at one while more collection than dispatch at another))
        // transport capacity works both ways (going there and returning)
        let mut dispatch_capacity = transportation_capacity;
        let mut collect_capacity = transportation_capacity;
        let mut missing_dispatch: f32 = 0.0;
        let mut missing_collect: f32 = 0.0;
        let mut result = GoodMap::default();
        const MIN_SELL_PRICE: f32 = 1.0;
        // value+amount per good
        let mut missing_goods: Vec<(GoodIndex, (f32, f32))> = self
            .surplus
            .iter()
            .filter(|(g, a)| (**a < 0.0 && *g != *TRANSPORTATION_INDEX))
            .map(|(g, a)| (g, (self.values[g].unwrap_or(Economy::MINIMUM_PRICE), -*a)))
            .collect();
        missing_goods.sort_by(|a, b| b.1.0.partial_cmp(&a.1.0).unwrap_or(Less));
        let mut extra_goods: GoodMap<f32> = GoodMap::from_iter(
            self.surplus
                .iter()
                .chain(core::iter::once((*COIN_INDEX, &self.stocks[*COIN_INDEX])))
                .filter(|(g, a)| (**a > 0.0 && *g != *TRANSPORTATION_INDEX))
                .map(|(g, a)| (g, *a)),
            0.0,
        );
        // ratio+price per good and site
        type GoodRatioPrice = Vec<(GoodIndex, (f32, f32))>;
        let good_payment: DHashMap<Id<Site>, GoodRatioPrice> = self
            .neighbors
            .iter()
            .map(|n| {
                let mut rel_value = extra_goods
                    .iter()
                    .map(|(g, _)| (g, n.last_values[g]))
                    .filter(|(_, last_val)| *last_val >= MIN_SELL_PRICE)
                    .map(|(g, last_val)| {
                        (
                            g,
                            (
                                last_val
                                    / self.values[g].unwrap_or(-1.0).max(Economy::MINIMUM_PRICE),
                                last_val,
                            ),
                        )
                    })
                    .collect::<Vec<_>>();
                rel_value.sort_by(|a, b| b.1.0.partial_cmp(&a.1.0).unwrap_or(Less));
                (n.id, rel_value)
            })
            .collect();
        // price+stock per site and good
        type SitePriceStock = Vec<(Id<Site>, (f32, f32))>;
        let mut good_price: DHashMap<GoodIndex, SitePriceStock> = missing_goods
            .iter()
            .map(|(g, _)| {
                (*g, {
                    let mut neighbor_prices: Vec<(Id<Site>, (f32, f32))> = self
                        .neighbors
                        .iter()
                        .filter(|n| n.last_supplies[*g] > 0.0)
                        .map(|n| (n.id, (n.last_values[*g], n.last_supplies[*g])))
                        .collect();
                    neighbor_prices.sort_by(|a, b| a.1.0.partial_cmp(&b.1.0).unwrap_or(Less));
                    neighbor_prices
                })
            })
            .collect();
        // TODO: we need to introduce priority (according to available transportation
        // capacity)
        let mut neighbor_orders: DHashMap<Id<Site>, GoodMap<f32>> = self
            .neighbors
            .iter()
            .map(|n| (n.id, GoodMap::default()))
            .collect();
        if site_id.id() == 1 {
            // cut down number of lines printed
            debug!(
                "Site {} #neighbors {} Transport capacity {}",
                site_id.id(),
                self.neighbors.len(),
                transportation_capacity,
            );
            debug!("missing {:#?} extra {:#?}", missing_goods, extra_goods,);
            debug!("buy {:#?} pay {:#?}", good_price, good_payment);
        }
        // === the actual planning is here ===
        for (g, (_, a)) in missing_goods.iter() {
            let mut amount = *a;
            if let Some(site_price_stock) = good_price.get_mut(g) {
                for (s, (price, supply)) in site_price_stock.iter_mut() {
                    // how much to buy, limit by supply and transport budget
                    let mut buy_target = amount.min(*supply);
                    let effort = transportation_effort(*g);
                    let collect = buy_target * effort;
                    let mut potential_balance: f32 = 0.0;
                    if collect > collect_capacity && effort > 0.0 {
                        let transportable_amount = collect_capacity / effort;
                        let missing_trade = buy_target - transportable_amount;
                        potential_trade[*g] += missing_trade;
                        potential_balance += missing_trade * *price;
                        buy_target = transportable_amount; // (buy_target - missing_trade).max(0.0); // avoid negative buy target caused by numeric inaccuracies
                        missing_collect += collect - collect_capacity;
                        debug!(
                            "missing capacity {:?}/{:?} {:?}",
                            missing_trade, amount, potential_balance,
                        );
                        amount = (amount - missing_trade).max(0.0); // you won't be able to transport it from elsewhere either, so don't count multiple times
                    }
                    let mut balance: f32 = *price * buy_target;
                    debug!(
                        "buy {:?} at {:?} amount {:?} balance {:?}",
                        *g,
                        s.id(),
                        buy_target,
                        balance,
                    );
                    if let Some(neighbor_orders) = neighbor_orders.get_mut(s) {
                        // find suitable goods in exchange
                        let mut acute_missing_dispatch: f32 = 0.0; // only count the highest priority (not multiple times)
                        for (g2, (_, price2)) in good_payment[s].iter() {
                            let mut amount2 = extra_goods[*g2];
                            // good available for trading?
                            if amount2 > 0.0 {
                                amount2 = amount2.min(balance / price2); // pay until balance is even
                                let effort2 = transportation_effort(*g2);
                                let mut dispatch = amount2 * effort2;
                                // limit by separate transport budget (on way back)
                                if dispatch > dispatch_capacity && effort2 > 0.0 {
                                    let transportable_amount = dispatch_capacity / effort2;
                                    let missing_trade = amount2 - transportable_amount;
                                    amount2 = transportable_amount;
                                    if acute_missing_dispatch == 0.0 {
                                        acute_missing_dispatch = missing_trade * effort2;
                                    }
                                    debug!(
                                        "can't carry payment {:?} {:?} {:?}",
                                        g2, dispatch, dispatch_capacity
                                    );
                                    dispatch = dispatch_capacity;
                                }

                                extra_goods[*g2] -= amount2;
                                debug!("pay {:?} {:?} = {:?}", g2, amount2, balance);
                                balance -= amount2 * price2;
                                neighbor_orders[*g2] -= amount2;
                                dispatch_capacity = (dispatch_capacity - dispatch).max(0.0);
                                if balance == 0.0 {
                                    break;
                                }
                            }
                        }
                        missing_dispatch += acute_missing_dispatch;
                        // adjust order if we are unable to pay for it
                        buy_target -= balance / *price;
                        buy_target = buy_target.min(amount);
                        collect_capacity = (collect_capacity - buy_target * effort).max(0.0);
                        neighbor_orders[*g] += buy_target;
                        amount -= buy_target;
                        debug!(
                            "deal amount {:?} end_balance {:?} price {:?} left {:?}",
                            buy_target, balance, *price, amount
                        );
                    }
                }
            }
        }
        // if site_id.id() == 1 {
        //     // cut down number of lines printed
        //     info!("orders {:#?}", neighbor_orders,);
        // }
        // TODO: Use planned orders and calculate value, stock etc. accordingly
        for n in &self.neighbors {
            if let Some(orders) = neighbor_orders.get(&n.id) {
                for (g, a) in orders.iter() {
                    result[g] += *a;
                }
                let to = TradeOrder {
                    customer: *site_id,
                    amount: *orders,
                };
                if let Some(o) = self.orders.get_mut(&n.id) {
                    // this is just to catch unbound growth (happened in development)
                    if o.len() < 100 {
                        o.push(to);
                    } else {
                        debug!("overflow {:?}", o);
                    }
                } else {
                    self.orders.insert(n.id, vec![to]);
                }
            }
        }
        // return missing transport capacity
        //missing_collect.max(missing_dispatch)
        debug!(
            "Tranportation {:?} {:?} {:?} {:?} {:?}",
            transportation_capacity,
            collect_capacity,
            dispatch_capacity,
            missing_collect,
            missing_dispatch,
        );
        result[*TRANSPORTATION_INDEX] = -(transportation_capacity
            - collect_capacity.min(dispatch_capacity)
            + missing_collect.max(missing_dispatch));
        if site_id.id() == 1 {
            debug!("Trade {:?}", result);
        }
        result
    }

    /// perform trade using neighboring orders (2nd step of trading)
    pub fn trade_at_site(
        &mut self,
        site_id: Id<Site>,
        orders: &mut Vec<TradeOrder>,
        // economy: &mut Economy,
        deliveries: &mut DHashMap<Id<Site>, Vec<TradeDelivery>>,
    ) {
        // make sure that at least this amount of stock remains available
        // TODO: rework using economy.unconsumed_stock

        let internal_orders = self.get_orders();
        let mut next_demand = GoodMap::from_default(0.0);
        for (labor, orders) in &internal_orders {
            let workers = if let Some(labor) = labor {
                self.labors[*labor]
            } else {
                1.0
            } * self.pop;
            for (good, amount) in orders {
                next_demand[*good] += *amount * workers;
                assert!(next_demand[*good] >= 0.0);
            }
        }
        //info!("Trade {} {}", site.id(), orders.len());
        let mut total_orders: GoodMap<f32> = GoodMap::from_default(0.0);
        for i in orders.iter() {
            for (g, &a) in i.amount.iter().filter(|(_, a)| **a > 0.0) {
                total_orders[g] += a;
            }
        }
        let order_stock_ratio: GoodMap<Option<f32>> = GoodMap::from_iter(
            self.stocks
                .iter()
                .map(|(g, a)| (g, *a, next_demand[g]))
                .filter(|(_, a, s)| *a > *s)
                .map(|(g, a, s)| (g, Some(total_orders[g] / (a - s)))),
            None,
        );
        debug!("trade {} {:?}", site_id.id(), order_stock_ratio);
        let prices = GoodMap::from_iter(
            self.values
                .iter()
                .map(|(g, o)| (g, o.unwrap_or(0.0).max(Economy::MINIMUM_PRICE))),
            0.0,
        );
        for o in orders.drain(..) {
            // amount, local value (sell low value, buy high value goods first (trading
            // town's interest))
            let mut sorted_sell: Vec<(GoodIndex, f32, f32)> = o
                .amount
                .iter()
                .filter(|(_, &a)| a > 0.0)
                .map(|(g, a)| (g, *a, prices[g]))
                .collect();
            sorted_sell.sort_by(|a, b| (a.2.partial_cmp(&b.2).unwrap_or(Less)));
            let mut sorted_buy: Vec<(GoodIndex, f32, f32)> = o
                .amount
                .iter()
                .filter(|(_, &a)| a < 0.0)
                .map(|(g, a)| (g, *a, prices[g]))
                .collect();
            sorted_buy.sort_by(|a, b| (b.2.partial_cmp(&a.2).unwrap_or(Less)));
            debug!(
                "with {} {:?} buy {:?}",
                o.customer.id(),
                sorted_sell,
                sorted_buy
            );
            let mut good_delivery = GoodMap::from_default(0.0);
            for (g, amount, price) in sorted_sell.iter() {
                if let Some(order_stock_ratio) = order_stock_ratio[*g] {
                    let allocated_amount = *amount / order_stock_ratio.max(1.0);
                    let mut balance = allocated_amount * *price;
                    for (g2, avail, price2) in sorted_buy.iter_mut() {
                        let amount2 = (-*avail).min(balance / *price2);
                        assert!(amount2 >= 0.0);
                        self.stocks[*g2] += amount2;
                        balance = (balance - amount2 * *price2).max(0.0);
                        *avail += amount2; // reduce (negative) brought stock
                        debug!("paid with {:?} {} {}", *g2, amount2, *price2);
                        if balance == 0.0 {
                            break;
                        }
                    }
                    let mut paid_amount =
                        (allocated_amount - balance / *price).min(self.stocks[*g]);
                    if paid_amount / allocated_amount < 0.95 {
                        debug!(
                            "Client {} is broke on {:?} : {} {} severity {}",
                            o.customer.id(),
                            *g,
                            paid_amount,
                            allocated_amount,
                            order_stock_ratio,
                        );
                    } else {
                        debug!("bought {:?} {} {}", *g, paid_amount, *price);
                    }
                    if self.stocks[*g] - paid_amount < 0.0 {
                        info!(
                            "BUG {:?} {:?} {} TO {:?} OSR {:?} ND {:?}",
                            self.stocks[*g],
                            *g,
                            paid_amount,
                            total_orders[*g],
                            order_stock_ratio,
                            next_demand[*g]
                        );
                        paid_amount = self.stocks[*g];
                    }
                    good_delivery[*g] += paid_amount;
                    self.stocks[*g] -= paid_amount;
                }
            }
            for (g, amount, _) in sorted_buy.drain(..) {
                if amount < 0.0 {
                    debug!("shipping back unsold {} of {:?}", amount, g);
                    good_delivery[g] += -amount;
                }
            }
            let delivery = TradeDelivery {
                supplier: site_id,
                prices,
                supply: GoodMap::from_iter(
                    self.stocks.iter().map(|(g, a)| {
                        (g, {
                            (a - next_demand[g] - total_orders[g]).max(0.0) + good_delivery[g]
                        })
                    }),
                    0.0,
                ),
                amount: good_delivery,
            };
            debug!(?delivery);
            if let Some(deliveries) = deliveries.get_mut(&o.customer) {
                deliveries.push(delivery);
            } else {
                deliveries.insert(o.customer, vec![delivery]);
            }
        }
        if !orders.is_empty() {
            info!("non empty orders {:?}", orders);
            orders.clear();
        }
    }

    /// 3rd step of trading
    fn collect_deliveries(
        // site: &mut Site,
        &mut self,
        // deliveries: &mut Vec<TradeDelivery>,
        // ctx: &mut vergleich::Context,
    ) {
        // collect all the goods we shipped
        let mut last_exports = GoodMap::from_iter(
            self.active_exports
                .iter()
                .filter(|(_g, a)| **a > 0.0)
                .map(|(g, a)| (g, *a)),
            0.0,
        );
        // TODO: properly rate benefits created by merchants (done below?)
        for mut d in self.deliveries.drain(..) {
            // let mut ictx = ctx.context(&format!("suppl {}", d.supplier.id()));
            for i in d.amount.iter() {
                last_exports[i.0] -= *i.1;
            }
            // remember price
            if let Some(n) = self.neighbors.iter_mut().find(|n| n.id == d.supplier) {
                // remember (and consume) last values
                std::mem::swap(&mut n.last_values, &mut d.prices);
                std::mem::swap(&mut n.last_supplies, &mut d.supply);
                // add items to stock
                for (g, a) in d.amount.iter() {
                    if *a < 0.0 {
                        // likely rounding error, ignore
                        debug!("Unexpected delivery for {:?} {}", g, *a);
                    } else {
                        self.stocks[g] += *a;
                    }
                }
            }
        }
        if !self.deliveries.is_empty() {
            info!("non empty deliveries {:?}", self.deliveries);
            self.deliveries.clear();
        }
        std::mem::swap(&mut last_exports, &mut self.last_exports);
        //self.active_exports.clear();
    }

    /// Simulate one step of economic interaction:
    /// - collect returned goods from trade
    /// - calculate demand, production and their ratio
    /// - reassign workers based on missing goods
    /// - change stock due to raw material use and production
    /// - send out traders with goods and orders
    /// - calculate good decay and population change
    ///
    /// Simulate a site's economy. This simulation is roughly equivalent to the
    /// Lange-Lerner model's solution to the socialist calculation problem. The
    /// simulation begins by assigning arbitrary values to each commodity and
    /// then incrementally updates them according to the final scarcity of
    /// the commodity at the end of the tick. This results in the
    /// formulation of values that are roughly analogous to prices for each
    /// commodity. The workforce is then reassigned according to the
    /// respective commodity values. The simulation also includes damping
    /// terms that prevent cyclical inconsistencies in value rationalisation
    /// magnifying enough to crash the economy. We also ensure that
    /// a small number of workers are allocated to every industry (even inactive
    /// ones) each tick. This is not an accident: a small amount of productive
    /// capacity in one industry allows the economy to quickly pivot to a
    /// different production configuration should an additional commodity
    /// that acts as production input become available. This means that the
    /// economy will dynamically react to environmental changes. If a
    /// product becomes available through a mechanism such as trade, an
    /// entire arm of the economy may materialise to take advantage of this.

    pub fn tick(
        &mut self,
        //deliveries: Option<&mut Vec<TradeDelivery>>,
        site_id: Id<Site>,
        dt: f32,
        mut vc: vergleich::Context,
    ) {
        // collect goods from trading
        if INTER_SITE_TRADE {
            // if let Some(deliveries) = deliveries {
            self.collect_deliveries();
            // }
        }

        let orders = self.get_orders();
        let productivity = self.get_productivity();

        for i in productivity.iter() {
            vc.context("productivity")
                .value(&std::format!("{:?}{:?}", i.0, Good::from(i.1.0)), i.1.1);
        }

        let mut demand = GoodMap::from_default(0.0);
        for (labor, orders) in &orders {
            let workers = if let Some(labor) = labor {
                self.labors[*labor]
            } else {
                1.0
            } * self.pop;
            for (good, amount) in orders {
                demand[*good] += *amount * workers;
            }
        }
        if INTER_SITE_TRADE {
            demand[*COIN_INDEX] += Economy::STARTING_COIN; // if we spend coin value increases
        }

        // which labor is the merchant
        let merchant_labor = productivity
            .iter()
            .find(|(_, v)| v.0 == *TRANSPORTATION_INDEX)
            .map(|(l, _)| l);

        let mut supply = self.stocks; //GoodMap::from_default(0.0);
        for (labor, goodvec) in productivity.iter() {
            //for (output_good, _) in goodvec.iter() {
            //info!("{} supply{:?}+={}", site_id.id(), Good::from(goodvec.0),
            // self.yields[labor] * self.labors[labor] * self.pop);
            supply[goodvec.0] += self.yields[labor] * self.labors[labor] * self.pop;
            vc.context(&std::format!("{:?}-{:?}", Good::from(goodvec.0), labor))
                .value("yields", self.yields[labor]);
            vc.context(&std::format!("{:?}-{:?}", Good::from(goodvec.0), labor))
                .value("labors", self.labors[labor]);
            //}
        }

        for i in supply.iter() {
            vc.context("supply")
                .value(&std::format!("{:?}", Good::from(i.0)), *i.1);
        }

        let stocks = &self.stocks;
        for i in stocks.iter() {
            vc.context("stocks")
                .value(&std::format!("{:?}", Good::from(i.0)), *i.1);
        }
        self.surplus = demand.map(|g, demand| supply[g] + stocks[g] - demand);
        self.marginal_surplus = demand.map(|g, demand| supply[g] - demand);

        // plan trading with other sites
        // let external_orders = &mut index.trade.orders;
        let mut potential_trade = GoodMap::from_default(0.0);
        // use last year's generated transportation for merchants (could we do better?
        // this is in line with the other professions)
        let transportation_capacity = self.stocks[*TRANSPORTATION_INDEX];
        let trade = if INTER_SITE_TRADE {
            let trade = self.plan_trade_for_site(
                &site_id,
                transportation_capacity,
                // external_orders,
                &mut potential_trade,
            );
            self.active_exports = GoodMap::from_iter(trade.iter().map(|(g, a)| (g, -*a)), 0.0); // TODO: check for availability?

            // add the wares to sell to demand and the goods to buy to supply
            for (g, a) in trade.iter() {
                vc.context("trade")
                    .value(&std::format!("{:?}", Good::from(g)), *a);
                if *a > 0.0 {
                    supply[g] += *a;
                    assert!(supply[g] >= 0.0);
                } else {
                    demand[g] -= *a;
                    assert!(demand[g] >= 0.0);
                }
            }
            trade
        } else {
            GoodMap::default()
        };

        // Update values according to the surplus of each stock
        // Note that values are used for workforce allocation and are not the same thing
        // as price
        // fall back to old (less wrong than other goods) coin logic
        let old_coin_surplus = self.stocks[*COIN_INDEX] - demand[*COIN_INDEX];
        let values = &mut self.values;

        self.surplus.iter().for_each(|(good, surplus)| {
            let old_surplus = if good == *COIN_INDEX {
                old_coin_surplus
            } else {
                *surplus
            };
            // Value rationalisation
            let goodname = std::format!("{:?}", Good::from(good));
            vc.context("old_surplus").value(&goodname, old_surplus);
            vc.context("demand").value(&goodname, demand[good]);
            let val = 2.0f32.powf(1.0 - old_surplus / demand[good]);
            let smooth = 0.8;
            values[good] = if val > 0.001 && val < 1000.0 {
                Some(vc.context("values").value(
                    &goodname,
                    smooth * values[good].unwrap_or(val) + (1.0 - smooth) * val,
                ))
            } else {
                None
            };
        });

        let all_trade_goods: DHashSet<GoodIndex> = trade
            .iter()
            .chain(potential_trade.iter())
            .filter(|(_, a)| **a > 0.0)
            .map(|(g, _)| g)
            .collect();
        //let empty_goods: DHashSet<GoodIndex> = DHashSet::default();
        // TODO: Does avg/max/sum make most sense for labors creating more than one good
        // summing favors merchants too much (as they will provide multiple
        // goods, so we use max instead)
        let labor_ratios: LaborMap<f32> = LaborMap::from_iter(
            productivity.iter().map(|(labor, goodvec)| {
                (
                    labor,
                    if Some(labor) == merchant_labor {
                        all_trade_goods
                            .iter()
                            .chain(std::iter::once(&goodvec.0))
                            .map(|&output_good| self.values[output_good].unwrap_or(0.0))
                            .max_by(|a, b| a.abs().partial_cmp(&b.abs()).unwrap_or(Less))
                    } else {
                        self.values[goodvec.0]
                    }
                    .unwrap_or(0.0)
                        * self.productivity[labor],
                )
            }),
            0.0,
        );
        debug!(?labor_ratios);

        let labor_ratio_sum = labor_ratios.iter().map(|(_, r)| *r).sum::<f32>().max(0.01);
        let mut labor_context = vc.context("labor");
        productivity.iter().for_each(|(labor, _)| {
            let smooth = 0.8;
            self.labors[labor] = labor_context.value(
                &format!("{:?}", labor),
                smooth * self.labors[labor]
                    + (1.0 - smooth)
                        * (labor_ratios[labor].max(labor_ratio_sum / 1000.0) / labor_ratio_sum),
            );
            assert!(self.labors[labor] >= 0.0);
        });

        // Production
        let stocks_before = self.stocks;
        // TODO: Should we recalculate demand after labor reassignment?

        let direct_use = direct_use_goods();
        // Handle the stocks you can't pile (decay)
        for g in direct_use {
            self.stocks[*g] = 0.0;
        }

        let mut total_labor_values = GoodMap::<f32>::default();
        // TODO: trade
        let mut total_outputs = GoodMap::<f32>::default();
        for (labor, orders) in orders.iter() {
            let workers = if let Some(labor) = labor {
                self.labors[*labor]
            } else {
                1.0
            } * self.pop;
            assert!(workers >= 0.0);
            let is_merchant = merchant_labor == *labor;

            // For each order, we try to find the minimum satisfaction rate - this limits
            // how much we can produce! For example, if we need 0.25 fish and
            // 0.75 oats to make 1 unit of food, but only 0.5 units of oats are
            // available then we only need to consume 2/3rds
            // of other ingredients and leave the rest in stock
            // In effect, this is the productivity
            let (labor_productivity, limited_by) = orders
                .iter()
                .map(|(good, amount)| {
                    // What quantity is this order requesting?
                    let _quantity = *amount * workers;
                    assert!(stocks_before[*good] >= 0.0);
                    assert!(demand[*good] >= 0.0);
                    // What proportion of this order is the economy able to satisfy?
                    ((stocks_before[*good] / demand[*good]).min(1.0), *good)
                })
                .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Less))
                .unwrap_or_else(|| {
                    panic!("Industry {:?} requires at least one input order", labor)
                });
            assert!(labor_productivity >= 0.0);
            if let Some(labor) = labor {
                self.limited_by[*labor] = if labor_productivity >= 1.0 {
                    GoodIndex::default()
                } else {
                    limited_by
                };
            }

            let mut total_materials_cost = 0.0;
            for (good, amount) in orders {
                // What quantity is this order requesting?
                let quantity = *amount * workers;
                // What amount gets actually used in production?
                let used = quantity * labor_productivity;

                // Material cost of each factor of production
                total_materials_cost += used * self.labor_values[*good].unwrap_or(0.0);

                // Deplete stocks accordingly
                if !direct_use.contains(good) {
                    self.stocks[*good] = (self.stocks[*good] - used).max(0.0);
                }
            }
            let mut produced_goods: GoodMap<f32> = GoodMap::from_default(0.0);
            if INTER_SITE_TRADE && is_merchant {
                // TODO: replan for missing merchant productivity???
                for (g, a) in trade.iter() {
                    if !direct_use.contains(&g) {
                        if *a < 0.0 {
                            // take these goods to the road
                            if self.stocks[g] + *a < 0.0 {
                                // we have a problem: Probably due to a shift in productivity we
                                // have less goods available than
                                // planned, so we would need to
                                // reduce the amount shipped
                                debug!("NEG STOCK {:?} {} {}", g, self.stocks[g], *a);
                                let reduced_amount = self.stocks[g];
                                let planned_amount: f32 = self
                                    .orders
                                    .iter()
                                    .map(|i| {
                                        i.1.iter()
                                            .filter(|o| o.customer == site_id)
                                            .map(|j| j.amount[g])
                                            .sum::<f32>()
                                    })
                                    .sum();
                                let scale = reduced_amount / planned_amount.abs();
                                debug!("re-plan {} {} {}", reduced_amount, planned_amount, scale);
                                for k in self.orders.iter_mut() {
                                    for l in k.1.iter_mut().filter(|o| o.customer == site_id) {
                                        l.amount[g] *= scale;
                                    }
                                }
                                self.stocks[g] = 0.0;
                            }
                            //                    assert!(self.stocks[g] + *a >= 0.0);
                            else {
                                self.stocks[g] += *a;
                            }
                        }
                        total_materials_cost += (-*a) * self.labor_values[g].unwrap_or(0.0);
                    } else {
                        // count on receiving these
                        produced_goods[g] += *a;
                    }
                }
                debug!(
                    "merchant {} {}: {:?} {} {:?}",
                    site_id.id(),
                    self.pop,
                    produced_goods,
                    total_materials_cost,
                    trade
                );
            }

            // Industries produce things
            if let Some(labor) = labor {
                let work_products = &productivity[*labor];
                //let workers = self.labors[*labor] * self.pop;
                //let final_rate = rate;
                //let yield_per_worker = labor_productivity;
                self.yields[*labor] = labor_productivity * work_products.1;
                self.productivity[*labor] = labor_productivity;
                //let total_product_rate: f32 = work_products.iter().map(|(_, r)| *r).sum();
                let (stock, rate) = work_products;
                let total_output = labor_productivity * *rate * workers;
                assert!(total_output >= 0.0);
                self.stocks[*stock] += total_output;
                produced_goods[*stock] += total_output;

                let produced_amount: f32 = produced_goods.iter().map(|(_, a)| *a).sum();
                for (stock, amount) in produced_goods.iter() {
                    let cost_weight = amount / produced_amount.max(0.001);
                    // Materials cost per unit
                    // TODO: How to handle this reasonably for multiple producers (collect upper and
                    // lower term separately)
                    self.material_costs[stock] =
                        total_materials_cost / amount.max(0.001) * cost_weight;
                    // Labor costs
                    let wages = 1.0;
                    let total_labor_cost = workers * wages;

                    total_labor_values[stock] +=
                        (total_materials_cost + total_labor_cost) * cost_weight;
                    total_outputs[stock] += amount;
                }
            }
        }

        // Update labour values per unit
        self.labor_values = total_labor_values.map(|stock, tlv| {
            let total_output = total_outputs[stock];
            if total_output > 0.01 {
                Some(tlv / total_output)
            } else {
                None
            }
        });

        // Decay stocks (the ones which totally decay are handled later)
        self.stocks
            .iter_mut()
            .map(|(c, v)| (v, 1.0 - decay_rate(c)))
            .for_each(|(v, factor)| *v *= factor);

        // Decay stocks
        self.replenish(dt);

        // Births/deaths
        const NATURAL_BIRTH_RATE: f32 = 0.05;
        const DEATH_RATE: f32 = 0.005;
        let population_growth = self.surplus[*FOOD_INDEX] > 0.0;
        let birth_rate = if population_growth {
            NATURAL_BIRTH_RATE
        } else {
            0.0
        };
        self.pop += vc.value(
            "pop",
            dt / DAYS_PER_YEAR * self.pop * (birth_rate - DEATH_RATE),
        );
        self.population_limited_by = if population_growth {
            GoodIndex::default()
        } else {
            *FOOD_INDEX
        };

        // calculate the new unclaimed stock
        //let next_orders = self.get_orders();
        // orders are static
        let mut next_demand = GoodMap::from_default(0.0);
        for (labor, orders) in orders.iter() {
            let workers = if let Some(labor) = labor {
                self.labors[*labor]
            } else {
                1.0
            } * self.pop;
            for (good, amount) in orders {
                next_demand[*good] += *amount * workers;
                assert!(next_demand[*good] >= 0.0);
            }
        }
        let mut us = vc.context("unconsumed");
        self.unconsumed_stock = GoodMap::from_iter(
            self.stocks.iter().map(|(g, a)| {
                (
                    g,
                    us.value(&format!("{:?}", Good::from(g)), *a - next_demand[g]),
                )
            }),
            0.0,
        );
    }
}

pub fn good_list() -> impl Iterator<Item = GoodIndex> {
    (0..GoodIndex::LENGTH).map(GoodIndex::from_usize)
}

// cache in GoodMap ?
pub fn transportation_effort(g: GoodIndex) -> f32 {
    match Good::from(g) {
        Terrain(_) | Territory(_) | RoadSecurity => 0.0,
        Coin => 0.01,
        Potions => 0.1,

        Armor => 2.0,
        Stone => 4.0,

        _ => 1.0,
    }
}

pub fn decay_rate(g: GoodIndex) -> f32 {
    match Good::from(g) {
        Food => 0.2,
        Flour => 0.1,
        Meat => 0.25,
        Ingredients => 0.1,
        _ => 0.0,
    }
}

/** you can't accumulate or save these options/resources for later */
pub fn direct_use_goods() -> &'static [GoodIndex] {
    lazy_static! {
        static ref DIRECT_USE: [GoodIndex; 13] = [
            GoodIndex::try_from(Transportation).unwrap_or_default(),
            GoodIndex::try_from(Territory(BiomeKind::Grassland)).unwrap_or_default(),
            GoodIndex::try_from(Territory(BiomeKind::Forest)).unwrap_or_default(),
            GoodIndex::try_from(Territory(BiomeKind::Lake)).unwrap_or_default(),
            GoodIndex::try_from(Territory(BiomeKind::Ocean)).unwrap_or_default(),
            GoodIndex::try_from(Territory(BiomeKind::Mountain)).unwrap_or_default(),
            GoodIndex::try_from(RoadSecurity).unwrap_or_default(),
            GoodIndex::try_from(Terrain(BiomeKind::Grassland)).unwrap_or_default(),
            GoodIndex::try_from(Terrain(BiomeKind::Forest)).unwrap_or_default(),
            GoodIndex::try_from(Terrain(BiomeKind::Lake)).unwrap_or_default(),
            GoodIndex::try_from(Terrain(BiomeKind::Ocean)).unwrap_or_default(),
            GoodIndex::try_from(Terrain(BiomeKind::Mountain)).unwrap_or_default(),
            GoodIndex::try_from(Terrain(BiomeKind::Desert)).unwrap_or_default(),
        ];
    }
    &*DIRECT_USE
}
