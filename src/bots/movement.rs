use super::{PlayerView, Pos, TilePoint, center, tile};
use crate::navigation::{Scratch, Tiles, Walkable};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};
struct LocalReach {
    start: TilePoint,
    extra: Option<TilePoint>,
    tiles: Arc<BTreeSet<TilePoint>>,
}
#[derive(Default)]
pub(super) struct Grid {
    pub(super) walkable: Walkable,
    pub(super) known: Arc<Tiles>,
    pub(super) dimensions: (i32, i32),
    reached: Mutex<Option<(TilePoint, Arc<BTreeSet<TilePoint>>)>>,
    local_reached: Mutex<Option<LocalReach>>,
    frontiers: Mutex<BTreeMap<TilePoint, bool>>,
    frontier_candidates: OnceLock<BTreeSet<TilePoint>>,
    candidates: AtomicU64,
    pub(super) performance: Arc<Mutex<crate::performance::Window>>,
    searches: AtomicU64,
    cache_hits: AtomicU64,
    reachable_nodes: AtomicU64,
    route_nodes: AtomicU64,
}
impl Clone for Grid {
    fn clone(&self) -> Self {
        // Obstacle edits keep terrain geometry, but invalidate component caches.
        Self {
            walkable: self.walkable.clone(),
            known: self.known.clone(),
            dimensions: self.dimensions,
            frontiers: Mutex::new(
                self.frontiers
                    .lock()
                    .expect("frontier cache poisoned")
                    .clone(),
            ),
            frontier_candidates: self.frontier_candidates.clone(),
            performance: self.performance.clone(),
            ..Self::default()
        }
    }
}
impl Grid {
    pub(super) fn terrain(v: &PlayerView) -> Self {
        assert_eq!(
            v.tiles.dimensions(),
            (v.map.width, v.map.height),
            "navigation view dimensions mismatch"
        );
        Self {
            walkable: Walkable::new(v.tiles.clone()),
            known: v.tiles.clone(),
            dimensions: v.tiles.dimensions(),
            ..Self::default()
        }
    }
    pub(super) fn invalidate(&mut self) {
        *self.reached.get_mut().expect("navigation cache poisoned") = None;
        *self
            .local_reached
            .get_mut()
            .expect("local navigation cache poisoned") = None;
        self.searches.store(0, Ordering::Relaxed);
        self.cache_hits.store(0, Ordering::Relaxed);
        self.reachable_nodes.store(0, Ordering::Relaxed);
        self.route_nodes.store(0, Ordering::Relaxed);
        self.candidates.store(0, Ordering::Relaxed);
    }
    pub(super) fn towards(&self, from: Pos, target: Pos) -> Option<Pos> {
        if self.walkable.contains(&tile(target)) {
            return self.next(from, target);
        }
        let reachable = self.reachable(from);
        let measuring = crate::performance::Measure::start();
        let next = self
            .frontier_tiles()
            .intersection(&reachable)
            .filter(|point| {
                self.candidates.fetch_add(1, Ordering::Relaxed);
                center(**point).distance(from) > 3.
            })
            .min_by(|a, b| {
                center(**a)
                    .distance(target)
                    .total_cmp(&center(**b).distance(target))
            });
        self.performance
            .lock()
            .expect("bot timings poisoned")
            .work("frontier_selection", measuring);
        self.next(from, center(*next?))
    }
    pub(super) fn reachable(&self, from: Pos) -> Arc<BTreeSet<TilePoint>> {
        let start = tile(from);
        let mut cache = self.reached.lock().expect("navigation cache poisoned");
        if let Some((origin, reached)) = &*cache
            && (*origin == start
                || (self.walkable.contains(origin)
                    && self.walkable.contains(&start)
                    && reached.contains(&start)))
        {
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
            return reached.clone();
        }
        let measuring = crate::performance::Measure::start();
        let mut queue = VecDeque::from([start]);
        let mut visited = Scratch::new(self.dimensions);
        visited.insert(start);
        let mut reached = vec![start];
        while let Some((x, y)) = queue.pop_front() {
            for next in [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)] {
                if self.walkable.contains(&next) && visited.insert(next) {
                    reached.push(next);
                    queue.push_back(next);
                }
            }
        }
        let reached: Arc<BTreeSet<TilePoint>> = Arc::new(reached.into_iter().collect());
        *cache = Some((start, reached.clone()));
        drop(cache);
        self.performance
            .lock()
            .expect("bot timings poisoned")
            .work("reachability", measuring);
        self.searches.fetch_add(1, Ordering::Relaxed);
        self.reachable_nodes
            .fetch_add(reached.len() as u64, Ordering::Relaxed);
        reached
    }
    pub(super) fn nearby_reachable(
        &self,
        from: Pos,
        extra: Option<TilePoint>,
    ) -> Arc<BTreeSet<TilePoint>> {
        let start = tile(from);
        let extra = extra
            .filter(|p| {
                (i32::from(p.0) - i32::from(start.0)).abs() > 24
                    || (i32::from(p.1) - i32::from(start.1)).abs() > 24
            })
            .filter(|p| self.walkable.contains(p));
        let mut cache = self
            .local_reached
            .lock()
            .expect("local navigation cache poisoned");
        if let Some(old) = &*cache
            && old.start == start
            && old.extra == extra
        {
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
            return old.tiles.clone();
        }
        let measuring = crate::performance::Measure::start();
        let mut targets = Scratch::new(self.dimensions);
        for x in start.0.saturating_sub(24)..=start.0.saturating_add(24) {
            for y in start.1.saturating_sub(24)..=start.1.saturating_add(24) {
                if self.walkable.contains(&(x, y)) {
                    targets.insert((x, y));
                }
            }
        }
        targets.insert(start);
        if let Some(point) = extra {
            targets.insert(point);
        }
        let mut remaining = targets.clone();
        remaining.remove(&start);
        let mut visited = Scratch::new(self.dimensions);
        visited.insert(start);
        let mut queue = VecDeque::from([start]);
        // Stop only when every requested tile is proven reachable, or the entire component is exhausted.
        while !remaining.is_empty()
            && let Some((x, y)) = queue.pop_front()
        {
            for next in [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)] {
                if self.walkable.contains(&next) && visited.insert(next) {
                    remaining.remove(&next);
                    queue.push_back(next);
                }
            }
        }
        let nodes = visited.len();
        let tiles: Arc<BTreeSet<_>> =
            Arc::new(targets.points().filter(|p| visited.contains(p)).collect());
        *cache = Some(LocalReach {
            start,
            extra,
            tiles: tiles.clone(),
        });
        drop(cache);
        self.searches.fetch_add(1, Ordering::Relaxed);
        self.reachable_nodes
            .fetch_add(nodes as u64, Ordering::Relaxed);
        let mut performance = self.performance.lock().expect("bot timings poisoned");
        performance.count("local_reachability_nodes", nodes as u64);
        performance.work("local_reachability", measuring);
        tiles
    }
    pub(super) fn nearby<'a>(
        &'a self,
        reachable: &'a BTreeSet<TilePoint>,
        from: Pos,
    ) -> impl Iterator<Item = TilePoint> + 'a {
        let (x, y) = tile(from);
        (x.saturating_sub(24)..=x.saturating_add(24))
            .flat_map(move |x| (y.saturating_sub(24)..=y.saturating_add(24)).map(move |y| (x, y)))
            .filter(move |t| {
                self.candidates.fetch_add(1, Ordering::Relaxed);
                reachable.contains(t)
            })
    }
    pub(super) fn frontier(&self, (x, y): TilePoint, width: i32, height: i32) -> bool {
        assert_eq!(
            (width, height),
            self.dimensions,
            "frontier dimensions changed"
        );
        assert!(
            x >= 0 && y >= 0 && i32::from(x) < width && i32::from(y) < height,
            "frontier outside map"
        );
        *self
            .frontiers
            .lock()
            .expect("frontier cache poisoned")
            .entry((x, y))
            .or_insert_with(|| {
                [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)]
                    .iter()
                    .any(|n| {
                        n.0 >= 0
                            && n.1 >= 0
                            && i32::from(n.0) < width
                            && i32::from(n.1) < height
                            && !self.known.contains(n)
                    })
            })
    }
    fn frontier_tiles(&self) -> &BTreeSet<TilePoint> {
        self.frontier_candidates.get_or_init(|| {
            let measuring = crate::performance::Measure::start();
            let candidates = self
                .known
                .points()
                .filter(|&(x, y)| {
                    [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)]
                        .iter()
                        .any(|n| {
                            n.0 >= 0
                                && n.1 >= 0
                                && i32::from(n.0) < self.dimensions.0
                                && i32::from(n.1) < self.dimensions.1
                                && !self.known.contains(n)
                        })
                })
                .collect();
            self.performance
                .lock()
                .expect("bot timings poisoned")
                .work("frontier_build", measuring);
            candidates
        })
    }
    pub(super) fn segment(&self, from: Pos, to: Pos) -> bool {
        let steps = (from.distance(to) * 4.).ceil().max(1.) as i32;
        self.walkable.contains(&tile(to))
            && (0..=steps).all(|i| {
                let f = i as f32 / steps as f32;
                self.walkable.contains(&tile(Pos {
                    x: from.x + (to.x - from.x) * f,
                    y: from.y + (to.y - from.y) * f,
                }))
            })
    }
    pub(super) fn metrics(&self) -> serde_json::Value {
        let reached = self.reached.lock().expect("navigation cache poisoned");
        serde_json::json!({"known_tiles":self.known.len(),"walkable_tiles":self.walkable.len(),
            "reachable_tiles":reached.as_ref().map_or(0, |(_,r)|r.len()),"local_reachable_tiles":self.local_reached.lock().expect("local navigation cache poisoned").as_ref().map_or(0,|r|r.tiles.len()),
            "frontier_entries":self.frontiers.lock().expect("frontier cache poisoned").len(),"frontier_candidates":self.frontier_candidates.get().map_or(0,BTreeSet::len),"candidate_tiles_inspected":self.candidates.load(Ordering::Relaxed),
            "searches":self.searches.load(Ordering::Relaxed),"cache_hits":self.cache_hits.load(Ordering::Relaxed),
            "reachable_nodes_visited":self.reachable_nodes.load(Ordering::Relaxed),"route_nodes_visited":self.route_nodes.load(Ordering::Relaxed)})
    }
    pub(super) fn next(&self, from: Pos, to: Pos) -> Option<Pos> {
        let start = tile(from);
        let finish = tile(to);
        if !self.walkable.contains(&finish) {
            return None;
        }
        // A distant sampling grid can miss the nearby waypoint's blocked cell.
        let fraction = 4. / from.distance(to).max(4.);
        let waypoint = Pos {
            x: from.x + (to.x - from.x) * fraction,
            y: from.y + (to.y - from.y) * fraction,
        };
        if self.segment(from, to) && self.segment(from, waypoint) {
            return Some(waypoint);
        }
        if start == finish {
            return None;
        }
        let measuring = crate::performance::Measure::start();
        let mut queue = VecDeque::from([start]);
        let mut parent = HashMap::from([(start, start)]);
        let mut visited = 0;
        while let Some((x, y)) = queue.pop_front() {
            visited += 1;
            if (x, y) == finish {
                break;
            }
            for next in [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)] {
                if self.walkable.contains(&next) && !parent.contains_key(&next) {
                    parent.insert(next, (x, y));
                    queue.push_back(next);
                }
            }
        }
        self.route_nodes.fetch_add(visited, Ordering::Relaxed);
        {
            let mut p = self.performance.lock().expect("bot timings poisoned");
            p.count("route_nodes", visited);
            p.work("routing", measuring);
        }
        let mut next = finish;
        while *parent.get(&next)? != start {
            next = *parent.get(&next)?;
        }
        let target = center(next);
        if self.segment(from, target) {
            Some(target)
        } else {
            self.segment(from, center(start)).then_some(center(start))
        }
    }
}
