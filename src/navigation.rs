use crate::world::view::Tile;
use std::{
    collections::BTreeMap,
    ops::Deref,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

type Point = (i16, i16);
const SIDE: usize = 64;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
fn identity() -> u64 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    assert_ne!(id, u64::MAX, "navigation identity exhausted");
    id
}

pub(crate) struct Geometry {
    dimensions: (i32, i32),
    ground: Vec<u8>,
    speed: [f32; 256],
}
impl Geometry {
    pub(crate) fn new(
        dimensions: (i32, i32),
        ground: Vec<u8>,
        speeds: &BTreeMap<u8, f32>,
    ) -> Arc<Self> {
        assert!(
            dimensions.0 > 0
                && dimensions.1 > 0
                && dimensions.0 <= i16::MAX.into()
                && dimensions.1 <= i16::MAX.into(),
            "invalid navigation dimensions"
        );
        assert_eq!(
            ground.len(),
            dimensions.0 as usize * dimensions.1 as usize,
            "navigation geometry size mismatch"
        );
        let mut speed = [f32::NAN; 256];
        for (&kind, &value) in speeds {
            speed[kind as usize] = value;
        }
        assert!(
            ground.iter().all(|&g| speed[g as usize].is_finite()),
            "navigation geometry has unknown ground"
        );
        Arc::new(Self {
            dimensions,
            ground,
            speed,
        })
    }
    fn tile(&self, (x, y): Point) -> Tile {
        let ground = self.ground[y as usize * self.dimensions.0 as usize + x as usize];
        Tile {
            x,
            y,
            ground,
            ground_speed: self.speed[ground as usize],
        }
    }
}

type Words = [u64; SIDE];
pub(crate) trait Page: Clone + Deref<Target = Words> {
    fn zero() -> Self;
    fn edit(&mut self) -> &mut Words;
}
impl Page for Arc<Words> {
    fn zero() -> Self {
        Arc::new([0; SIDE])
    }
    fn edit(&mut self) -> &mut Words {
        Arc::make_mut(self)
    }
}
impl Page for Box<Words> {
    fn zero() -> Self {
        Box::new([0; SIDE])
    }
    fn edit(&mut self) -> &mut Words {
        self
    }
}
// Only retained knowledge needs copy-on-write; disposable search pages must not pay atomic ownership costs per tile.
pub(crate) type Mask = Bits<Arc<Words>>;
pub(crate) type Scratch = Bits<Box<Words>>;
#[derive(Clone)]
pub(crate) struct Bits<P: Page> {
    dimensions: (i32, i32),
    pages: Vec<Option<P>>,
    len: usize,
}
impl<P: Page> Default for Bits<P> {
    fn default() -> Self {
        Self {
            dimensions: (0, 0),
            pages: vec![],
            len: 0,
        }
    }
}
impl<P: Page> Bits<P> {
    pub(crate) fn new(dimensions: (i32, i32)) -> Self {
        Self {
            dimensions,
            pages: vec![
                None;
                (dimensions.0 as usize).div_ceil(SIDE)
                    * (dimensions.1 as usize).div_ceil(SIDE)
            ],
            len: 0,
        }
    }
    fn address(&self, (x, y): Point) -> Option<(usize, usize, u64)> {
        if x < 0 || y < 0 || i32::from(x) >= self.dimensions.0 || i32::from(y) >= self.dimensions.1
        {
            return None;
        }
        let (x, y) = (x as usize, y as usize);
        Some((
            x / SIDE * (self.dimensions.1 as usize).div_ceil(SIDE) + y / SIDE,
            x % SIDE,
            1 << (y % SIDE),
        ))
    }
    pub(crate) fn contains(&self, point: &Point) -> bool {
        self.address(*point).is_some_and(|(page, word, bit)| {
            self.pages[page]
                .as_ref()
                .is_some_and(|p| p[word] & bit != 0)
        })
    }
    pub(crate) fn insert(&mut self, point: Point) -> bool {
        let (page, word, bit) = self
            .address(point)
            .expect("knowledge outside navigation geometry");
        let p = self.pages[page].get_or_insert_with(P::zero);
        if p[word] & bit != 0 {
            return false;
        }
        p.edit()[word] |= bit;
        self.len += 1;
        true
    }
    pub(crate) fn remove(&mut self, point: &Point) -> bool {
        let Some((page, word, bit)) = self.address(*point) else {
            return false;
        };
        let Some(p) = self.pages[page].as_mut() else {
            return false;
        };
        if p[word] & bit == 0 {
            return false;
        }
        p.edit()[word] &= !bit;
        self.len -= 1;
        true
    }
    pub(crate) fn len(&self) -> usize {
        self.len
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub(crate) fn points(&self) -> impl Iterator<Item = Point> + '_ {
        // Column words retain the observation API's x/y ordering without sorting snapshots.
        let tall = (self.dimensions.1 as usize).div_ceil(SIDE).max(1);
        self.pages
            .chunks(tall)
            .enumerate()
            .filter(|(_, column)| column.iter().any(Option::is_some))
            .flat_map(move |(cx, column)| {
                (0..SIDE).flat_map(move |x| {
                    column.iter().enumerate().flat_map(move |(cy, page)| {
                        let mut bits = page.as_ref().map_or(0, |p| p[x]);
                        std::iter::from_fn(move || {
                            if bits == 0 {
                                return None;
                            }
                            let y = bits.trailing_zeros() as usize;
                            bits &= bits - 1;
                            Some(((cx * SIDE + x) as i16, (cy * SIDE + y) as i16))
                        })
                    })
                })
            })
    }
    pub(crate) fn allocated_bytes(&self) -> usize {
        self.pages.capacity() * std::mem::size_of::<Option<P>>()
            + self.pages.iter().filter(|p| p.is_some()).count() * SIDE * 8
    }
}

#[derive(Clone, Default)]
pub(crate) struct Tiles {
    geometry: Option<Arc<Geometry>>,
    known: Mask,
    id: u64,
    revision: u64,
    walkable: usize,
}
impl Tiles {
    pub(crate) fn new(geometry: Arc<Geometry>, points: impl IntoIterator<Item = Point>) -> Self {
        let mut result = Self {
            known: Mask::new(geometry.dimensions),
            geometry: Some(geometry.clone()),
            id: identity(),
            ..Self::default()
        };
        result.discover(&geometry, points);
        result
    }
    pub(crate) fn discover(
        &mut self,
        authority: &Arc<Geometry>,
        points: impl IntoIterator<Item = Point>,
    ) {
        let geometry = self
            .geometry
            .as_ref()
            .expect("navigation knowledge not initialized");
        // Snapshots cannot grant themselves more knowledge: only the world owns this capability.
        assert!(
            Arc::ptr_eq(geometry, authority),
            "navigation discovery authority mismatch"
        );
        for point in points {
            assert!(self.known.insert(point), "navigation tile discovered twice");
            self.walkable += usize::from(geometry.tile(point).ground_speed > 0.);
        }
        self.revision = self
            .revision
            .checked_add(1)
            .expect("navigation revision exhausted");
    }
    pub(crate) fn version(&self) -> (u64, u64) {
        (self.id, self.revision)
    }
    pub(crate) fn len(&self) -> usize {
        self.known.len()
    }
    pub(crate) fn contains(&self, point: &Point) -> bool {
        self.known.contains(point)
    }
    pub(crate) fn points(&self) -> impl Iterator<Item = Point> + '_ {
        self.known.points()
    }
    pub(crate) fn iter(&self) -> impl Iterator<Item = Tile> + '_ {
        self.known.points().map(|p| {
            self.geometry
                .as_ref()
                .expect("known terrain has no geometry")
                .tile(p)
        })
    }
    pub(crate) fn dimensions(&self) -> (i32, i32) {
        self.geometry
            .as_ref()
            .expect("navigation knowledge not initialized")
            .dimensions
    }
    pub(crate) fn allocated_bytes(&self) -> usize {
        self.known.allocated_bytes()
    }
    fn walkable(&self, p: &Point) -> bool {
        // Geometry is shared, but unseen cells must not influence any navigation answer.
        self.contains(p)
            && self
                .geometry
                .as_ref()
                .expect("known terrain has no geometry")
                .tile(*p)
                .ground_speed
                > 0.
    }
}
impl serde::Serialize for Tiles {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter())
    }
}

#[derive(Clone, Default)]
pub(crate) struct Walkable {
    pub(crate) terrain: Arc<Tiles>,
    blocked: Mask,
}
impl Walkable {
    pub(crate) fn new(terrain: Arc<Tiles>) -> Self {
        Self {
            blocked: Mask::new(terrain.dimensions()),
            terrain,
        }
    }
    pub(crate) fn contains(&self, p: &Point) -> bool {
        self.terrain.walkable(p) && !self.blocked.contains(p)
    }
    pub(crate) fn remove(&mut self, p: &Point) -> bool {
        self.terrain.walkable(p) && self.blocked.insert(*p)
    }
    pub(crate) fn insert(&mut self, p: Point) {
        assert!(
            self.terrain.walkable(&p),
            "restoring unknown or impassable terrain"
        );
        self.blocked.remove(&p);
    }
    pub(crate) fn len(&self) -> usize {
        self.terrain.walkable - self.blocked.len()
    }
}
