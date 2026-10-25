use super::*;
use crate::{condition, encounter::Motion};

#[derive(Clone, Copy, Debug)]
pub(super) struct Charge {
    goal: Option<Pos>,
    until: u64,
    ready: u64,
}
impl Charge {
    pub(super) fn suspend(&mut self, delta: u64) {
        self.until += delta;
        self.ready += delta;
    }
}
impl World {
    pub(super) fn charge_goal(
        &mut self,
        id: i32,
        target: Option<Pos>,
        motion: Motion,
        speed: f32,
    ) -> Option<Pos> {
        let Motion::Charge {
            sight,
            cooldown,
            delay,
            stop,
            ..
        } = motion
        else {
            panic!("charge planner received another movement kind")
        };
        let e = self.entities.get_mut(&id).expect("charging NPC missing");
        let pos = e.pos;
        let now = self.now;
        let slowed = e.conditions(now) & condition::bit(condition::SLOWED) != 0;
        let e = &mut **e;
        let state = e.charge.get_or_insert(Charge {
            goal: None,
            until: 0,
            ready: now + delay,
        });
        if let Some(goal) = state.goal {
            if pos.distance(goal) <= stop || now >= state.until {
                tracing::debug!(
                    entity = id,
                    arrived = pos.distance(goal) <= stop,
                    "locked charge ended"
                );
                state.goal = None;
                state.ready = now + cooldown;
                e.goal = None;
            } else {
                return Some(goal);
            }
        }
        if now >= state.ready
            && let Some(goal) =
                target.filter(|p| pos.distance(*p) > stop && pos.distance(*p) < sight)
        {
            let velocity = speed / 1.5 * if slowed { 0.5 } else { 1. };
            assert!(velocity > 0., "charge requires positive speed");
            state.goal = Some(goal);
            state.until =
                now + ((pos.distance(goal) - stop) / velocity * 1000.).ceil() as u64 + 100;
            tracing::debug!(
                entity = id,
                x = goal.x,
                y = goal.y,
                until = state.until,
                "charge captured target position"
            );
            return Some(goal);
        }
        None
    }
}
