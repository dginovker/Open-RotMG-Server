use super::*;

impl World {
    pub(super) fn room_sight(&self, center: (i32, i32)) -> BTreeSet<(i16, i16)> {
        // Reveal the enclosing wall face, not unopened rooms: https://www.youtube.com/watch?v=gWLn80rNsxU&t=30
        let mut seen = BTreeSet::from([(center.0 as i16, center.1 as i16)]);
        let mut queue = VecDeque::from([center]);
        while let Some((x, y)) = queue.pop_front() {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (x, y) = (x + dx, y + dy);
                    if x < 0
                        || y < 0
                        || x >= self.map.width
                        || y >= self.map.height
                        || (x - center.0).pow(2) + (y - center.1).pow(2) > 20 * 20
                        || !seen.insert((x as i16, y as i16))
                    {
                        continue;
                    }
                    if self.map.cells[(y * self.map.width + x) as usize].ground != 0xff
                        && !self
                            .entities_at(Pos {
                                x: x as f32 + 0.5,
                                y: y as f32 + 0.5,
                            })
                            .any(|e| self.content.objects[&e.kind].blocks_sight)
                    {
                        queue.push_back((x, y));
                    }
                }
            }
        }
        seen
    }

    pub(super) fn invalidate_sight(&mut self) {
        if self.name == "Snake Pit" {
            for p in self.players.values_mut() {
                p.tile_scan = None;
            }
        }
    }
}
