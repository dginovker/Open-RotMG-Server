use super::*;

fn tiles_per_second(b: &Behavior) -> f64 {
    // Release 7.0 MoveToward behaviors use speed * dt * 5. Buzz and Swirl use momentum rates.
    n(b, F::Speed)
        * if matches!(b.kind, K::Buzz | K::Swirl) {
            1.
        } else {
            5.
        }
}
fn stop(b: &Behavior) -> f64 {
    n(b, F::Range)
}
fn steer(current: f64, target: f64, limit: f64) -> f64 {
    let d = (target - current + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)
        - std::f64::consts::PI;
    current + d.clamp(-limit, limit)
}
pub(super) fn closing_phase(current: bool, distance: f64, outer: f64, inner: f64) -> bool {
    if distance <= inner {
        false
    } else if distance > outer {
        true
    } else {
        current
    }
}
pub(super) fn orbit_radius(
    base: f64,
    variability: f64,
    rng: &mut rand::rngs::StdRng,
) -> Result<f64> {
    let upper = base * (1. + variability);
    ensure!(
        base.is_finite()
            && base >= 0.
            && variability.is_finite()
            && variability >= 0.
            && upper.is_finite()
            && upper <= f32::MAX as f64,
        "unrepresentable Orbit radial envelope"
    );
    if base == 0. || variability == 0. {
        return Ok(base);
    }
    // The nonnegative uniform support for variability >1 is authored; no sampled negative tail is folded or clamped.
    let lower = if variability <= 1. { -variability } else { -1. };
    Ok(base * (1. + rng.gen_range(lower..=variability)))
}
pub(super) fn orbit_wobble(base: f64, amplitude: f64, period: u64, elapsed: u64) -> Result<f64> {
    ensure!(
        base.is_finite()
            && base >= 0.
            && amplitude.is_finite()
            && amplitude >= 0.
            && (base + amplitude).is_finite()
            && base + amplitude <= f32::MAX as f64,
        "unrepresentable Orbit wobble envelope"
    );
    if amplitude == 0. {
        return Ok(base);
    }
    ensure!(
        period > 0,
        "positive Orbit wobble requires compiled positive period"
    );
    let phase = std::f64::consts::TAU * (elapsed % period) as f64 / period as f64;
    Ok((base + amplitude * phase.sin()).abs())
}
impl World {
    pub(super) fn xml_owned_anchor(&self, actor: &Entity, kinds: &[u16]) -> Option<&Entity> {
        let mut child = actor;
        while let Some(parent) = child.parent.and_then(|id| self.entities.get(&id)) {
            // Summoned ancestors precede their descendants; skip invalid ownership cycles.
            if parent.id >= child.id {
                break;
            }
            if parent.hp > 0 && kinds.contains(&parent.kind) {
                return Some(parent);
            }
            child = parent;
        }
        None
    }
    pub(super) fn xml_anchor(&self, b: &Behavior, scope: Scope) -> Result<Option<i32>> {
        let kinds = objects(b, F::ProtecteeId);
        if let Some(parent) = self.xml_owned_anchor(self.xml_scope(scope)?, kinds) {
            return Ok(Some(parent.id));
        }
        Ok(self
            .xml_members(scope, kinds, n(b, F::AcquireRange))?
            .first()
            .copied())
    }
    fn xml_motion_target_range(&self, b: &Behavior) -> f64 {
        if b.kind == K::Shadow {
            f64::from(self.map.width).hypot(f64::from(self.map.height))
        } else {
            match b.kind {
                K::Follow | K::Buzz | K::Circle | K::KeepDistance | K::Orbit | K::Protect => {
                    n(b, F::AcquireRange)
                }
                K::Charge => n(b, F::Range),
                K::StayBack => n(b, F::Distance),
                _ => 20.,
            }
        }
    }
    pub(super) fn xml_shadow_teleport(&mut self, b: &Behavior, step: Step) -> Result<bool> {
        let actor = self.xml_scope(step.token.scope)?;
        let (id, pos) = (actor.id, actor.pos);
        if actor.conditions(self.now) & crate::condition::bit(crate::condition::PARALYZED) != 0 {
            return Ok(false);
        }
        let Some((_, target)) =
            self.xml_nearest_hostile_target(pos, 0., self.xml_motion_target_range(b))
        else {
            return Ok(false);
        };
        if pos.distance(target) <= 30. || !self.xml_teleport_endpoint(step.token.scope, target)? {
            return Ok(false);
        }
        self.position_entity(id, target)?;
        self.entities
            .get_mut(&id)
            .expect("teleported Shadow actor")
            .last_step = Pos {
            x: target.x - pos.x,
            y: target.y - pos.y,
        };
        self.xml.motions.remove(&step.token);
        Ok(true)
    }
    pub(super) fn xml_motion_ready(&self, b: &Behavior, step: Step) -> Result<bool> {
        let e = self.xml_scope(step.token.scope)?;
        if e.conditions(self.now) & crate::condition::bit(crate::condition::PARALYZED) != 0 {
            return Ok(false);
        }
        let distance = || {
            self.xml_nearest_hostile_target(e.pos, 0., self.xml_motion_target_range(b))
                .map(|(_, pos)| f64::from(e.pos.distance(pos)))
        };
        Ok(match b.kind {
            K::Follow | K::Charge | K::Buzz | K::Circle => distance().is_some(),
            K::Shadow => distance().is_some_and(|d| d > n(b, F::Range) || d > 30.),
            K::KeepDistance => distance().is_some_and(|d| (d - n(b, F::Distance)).abs() > 0.05),
            K::StayBack => distance().is_some_and(|d| d < n(b, F::Distance)),
            K::StayCloseToSpawn => f64::from(e.pos.distance(e.home)) > n(b, F::Range),
            K::Orbit => self.xml_anchor(b, step.token.scope)?.is_some(),
            K::Protect => self.xml_anchor(b, step.token.scope)?.is_some_and(|a| {
                f64::from(e.pos.distance(self.entities[&a].pos)) > n(b, F::ProtectionRange)
            }),
            K::StayAbove | K::StayInTerrain | K::Walk => self
                .xml_environment_goal(b, e.pos, e.kind, step.token.scope)?
                .is_some(),
            _ => true,
        })
    }
    pub(super) fn xml_start_motion(
        &mut self,
        b: &Behavior,
        step: Step,
        rng: &mut rand::rngs::StdRng,
    ) -> Result<bool> {
        let e = self.xml_scope(step.token.scope)?;
        let pos = e.pos;
        let target = if matches!(b.kind, K::Charge | K::Buzz | K::Swirl) {
            self.xml_nearest_hostile_target(pos, 0., self.xml_motion_target_range(b))
        } else {
            None
        };
        let (angle, wander_remaining) = match b.kind {
            K::Wander => {
                // Authored cadence from Kithio d15f321edcce0ef890001890c6991c25a1b0f718, wServer/logic/behaviors/Wander.cs.
                let angle = loop {
                    let x = rng.gen_range(-1..=1);
                    let y = rng.gen_range(-1..=1);
                    if (x, y) != (0, 0) {
                        break f64::from(y).atan2(f64::from(x));
                    }
                };
                (angle, f64::from(rng.gen_range(300..=700)) / 1000.)
            }
            K::BackAndForth | K::Move => (n(b, F::Angle), 0.),
            _ => (target.map_or(0., |(_, p)| heading(pos, p)), 0.),
        };
        let goal = if b.kind == K::Charge {
            let Some((_, goal)) = target else {
                return Ok(false);
            };
            goal
        } else if b.kind == K::Wander {
            pos
        } else {
            point(
                pos,
                angle,
                match b.kind {
                    K::Move => n(b, F::Distance),
                    K::BackAndForth => n(b, F::Radius),
                    _ => 10.,
                },
            )
        };
        ensure!(
            goal.x.is_finite() && goal.y.is_finite(),
            "unrepresentable native motion goal x={} y={} angle={angle}",
            goal.x,
            goal.y
        );
        let duration = if matches!(b.kind, K::Follow | K::Wander) {
            match value(b, F::Duration) {
                Some(Value::Millis(0)) if b.kind == K::Follow => None,
                Some(Value::Millis(duration)) => Some(*duration),
                None if b.kind == K::Wander => None,
                _ => bail!("{:?} missing or invalid compiled Duration", b.kind),
            }
        } else {
            None
        };
        let mut radius = if b.kind == K::Orbit {
            n(b, F::Radius)
        } else {
            0.
        };
        let mut speed = tiles_per_second(b);
        if b.kind == K::Orbit {
            let rv = n(b, F::RadiusVariability);
            let upper = radius * (1. + rv);
            ensure!(
                radius.is_finite()
                    && radius >= 0.
                    && rv.is_finite()
                    && rv >= 0.
                    && upper.is_finite()
                    && upper <= f32::MAX as f64,
                "unrepresentable Orbit radial envelope before sampling"
            );
            let sv = n(b, F::SpeedVariability);
            if sv > 0. {
                speed *= 1. + rng.gen_range(-sv..=sv);
            }
            radius = orbit_radius(radius, rv, rng)?;
        }
        ensure!(
            speed.is_finite() && speed >= 0. && radius.is_finite() && radius >= 0.,
            "invalid native motion units"
        );
        self.xml.motions.insert(
            step.token,
            Motion {
                goal,
                origin: pos,
                speed,
                angle,
                radius,
                wander_remaining,
                direction: if b.kind == K::BackAndForth
                    || (b.kind == K::Orbit && flag(b, F::Clockwise))
                {
                    1.
                } else {
                    -1.
                },
                anchor: if matches!(b.kind, K::Protect | K::Orbit) {
                    self.xml_anchor(b, step.token.scope)?
                } else {
                    None
                },
                closing: false,
                born: step.active_ms,
                until: duration
                    .map(|d| {
                        step.active_ms
                            .checked_add(d)
                            .context("motion deadline overflow")
                    })
                    .transpose()?,
            },
        );
        Ok(true)
    }
    pub(super) fn xml_move(&mut self, b: &Behavior, step: Step) -> Result<bool> {
        let Some(mut m) = self.xml.motions.get(&step.token).cloned() else {
            return Ok(false);
        };
        let e = self.xml_scope(step.token.scope)?;
        let (id, pos, kind) = (e.id, e.pos, e.kind);
        if m.until.is_some_and(|at| step.active_ms >= at) {
            self.xml.motions.remove(&step.token);
            return Ok(false);
        }
        let conditions = e.conditions(self.now);
        if conditions & crate::condition::bit(crate::condition::PARALYZED) != 0 {
            return Ok(false);
        }
        let elapsed = step.delta_ms as f64 / 1000.;
        let mut speed = m.speed;
        if b.fields.contains_key(&F::Acceleration) {
            speed = speed.min(n(b, F::Acceleration) * (step.active_ms - m.born) as f64 / 1000.);
        }
        if conditions & crate::condition::bit(crate::condition::SLOWED) != 0 {
            speed *= 0.5;
        } else if conditions & crate::condition::bit(crate::condition::SPEEDY) != 0 {
            speed *= 1.5;
        }
        let mut goal = m.goal;
        match b.kind {
            K::Follow | K::Shadow | K::Buzz | K::Circle | K::KeepDistance | K::StayBack => {
                let Some((player, mut target)) =
                    self.xml_nearest_hostile_target(pos, 0., self.xml_motion_target_range(b))
                else {
                    self.xml.motions.remove(&step.token);
                    return Ok(false);
                };
                // Decoy targets have no observed player motion to lead.
                if b.kind == K::Follow
                    && flag(b, F::Predictive)
                    && let Some(p) = self.players.get(&player)
                {
                    target.x += p.motion.velocity.x * 0.2;
                    target.y += p.motion.velocity.y * 0.2;
                }
                match b.kind {
                    K::Shadow => {
                        let distance = f64::from(pos.distance(target));
                        if m.anchor != Some(player) {
                            m.anchor = Some(player);
                            m.closing = distance > n(b, F::Range);
                        }
                        if distance > 30. {
                            if step.delta_ms == 0 {
                                return self.xml_teleport_endpoint(step.token.scope, target);
                            }
                            return self.xml_shadow_teleport(b, step);
                        }
                        if distance <= n(b, F::ReshadowRange) {
                            self.xml.motions.remove(&step.token);
                            return Ok(false);
                        }
                        if distance > n(b, F::Range) {
                            m.closing = true;
                        }
                        if !m.closing {
                            return Ok(false);
                        }
                        speed *= distance / n(b, F::Range);
                        goal = point(target, heading(target, pos), n(b, F::ReshadowRange));
                    }
                    K::Follow => {
                        let gap = stop(b);
                        if f64::from(pos.distance(target)) <= gap {
                            return Ok(false);
                        }
                        goal = point(target, heading(target, pos), gap);
                    }
                    K::Buzz => {
                        m.angle = steer(
                            m.angle,
                            heading(pos, target),
                            n(b, F::TurnRate).abs() * elapsed,
                        );
                        goal = point(pos, m.angle, (speed * elapsed).max(1.));
                    }
                    K::Circle => {
                        let radius = n(b, F::Distance);
                        goal = if radius == 0. {
                            target
                        } else {
                            let angle = heading(target, pos) + speed * elapsed / radius;
                            ensure!(angle.is_finite(), "Circle angular displacement overflow");
                            point(target, angle, radius)
                        };
                    }
                    K::KeepDistance => {
                        goal = point(target, heading(target, pos), n(b, F::Distance))
                    }
                    K::StayBack => {
                        if f64::from(pos.distance(target)) >= n(b, F::Distance) {
                            return Ok(false);
                        }
                        goal = point(target, heading(target, pos), n(b, F::Distance));
                    }
                    _ => unreachable!(),
                }
            }
            K::Protect | K::Orbit => {
                let anchor = m
                    .anchor
                    .filter(|id| self.entities.get(id).is_some_and(|e| e.hp > 0));
                let anchor = match anchor {
                    Some(id) => Some(id),
                    None => self.xml_anchor(b, step.token.scope)?,
                };
                let Some(anchor) = anchor else {
                    self.xml.motions.remove(&step.token);
                    return Ok(false);
                };
                let changed = m.anchor != Some(anchor);
                m.anchor = Some(anchor);
                let center = self.entities[&anchor].pos;
                if b.kind == K::Protect {
                    let radius = n(b, F::ReprotectRange);
                    if f64::from(pos.distance(center)) <= radius {
                        self.xml.motions.remove(&step.token);
                        return Ok(false);
                    }
                    goal = point(center, heading(center, pos), radius);
                } else {
                    let distance = f64::from(pos.distance(center));
                    let reprotect = opt_n(b, F::ReprotectRange);
                    if let (Some(outer), Some(inner)) = (opt_n(b, F::ProtectionRange), reprotect) {
                        if changed {
                            m.closing = distance > outer;
                        }
                        m.closing = closing_phase(m.closing, distance, outer, inner);
                    }
                    let radius = orbit_wobble(
                        m.radius,
                        n(b, F::RadiusWobbleDistance),
                        ms(b, F::RadiusWobblePeriod),
                        step.active_ms
                            .checked_sub(m.born)
                            .context("Orbit wobble clock regressed")?,
                    )?;
                    goal = if let Some(inner) = reprotect.filter(|_| m.closing) {
                        point(center, heading(center, pos), inner)
                    } else if radius == 0. {
                        center
                    } else {
                        let angle = heading(center, pos) + m.direction * speed * elapsed / radius;
                        ensure!(angle.is_finite(), "Orbit angular displacement overflow");
                        point(center, angle, radius)
                    };
                }
            }
            K::StayCloseToSpawn => goal = e.home,
            K::BackAndForth => {
                if pos.distance(goal) < 0.01 {
                    m.direction = -m.direction;
                }
                m.angle += n(b, F::TurnRate) * elapsed;
                goal = point(m.origin, m.angle, n(b, F::Radius) * m.direction);
                m.goal = goal;
            }
            K::Swirl => {
                m.angle += n(b, F::TurnRate) * elapsed;
                goal = point(pos, m.angle, n(b, F::Distance));
            }
            K::StayAbove | K::StayInTerrain | K::Walk => {
                let Some(p) = self.xml_environment_goal(b, pos, kind, step.token.scope)? else {
                    self.xml.motions.remove(&step.token);
                    return Ok(false);
                };
                goal = p;
            }
            _ => {}
        }
        let proposed = speed * elapsed;
        ensure!(
            proposed.is_finite(),
            "native motion distance overflow speed={speed} elapsed={elapsed}"
        );
        let (angle, limit) = if b.kind == K::Wander {
            (m.angle, None)
        } else {
            let remaining =
                (f64::from(goal.x) - f64::from(pos.x)).hypot(f64::from(goal.y) - f64::from(pos.y));
            ensure!(
                remaining.is_finite(),
                "nonfinite native motion remaining distance"
            );
            (heading(pos, goal), Some(remaining))
        };
        let step_distance = |requested: f64| limit.map_or(requested, |r| requested.min(r));
        let distance = step_distance(proposed);
        if speed == 0.
            && m.speed > 0.
            && step.active_ms == m.born
            && b.fields.contains_key(&F::Acceleration)
            && n(b, F::Acceleration) > 0.
        {
            let probe_speed = m.speed.min(n(b, F::Acceleration) * 0.05);
            let candidate = point(pos, angle, step_distance(probe_speed * 0.05));
            ensure!(
                candidate.x.is_finite() && candidate.y.is_finite(),
                "unrepresentable initial accelerating motion"
            );
            return self.xml_npc_walkable(step.token.scope, candidate);
        }
        if limit.is_some_and(|r| r <= 0.00001) || speed == 0. {
            return Ok(false);
        }
        if step.delta_ms == 0 {
            if b.kind == K::BackAndForth {
                return Ok(true);
            }
            let next = point(pos, angle, step_distance(speed * 0.05));
            ensure!(
                next.x.is_finite() && next.y.is_finite(),
                "unrepresentable native motion probe x={} y={}",
                next.x,
                next.y
            );
            return Ok(self.xml_npc_walkable(step.token.scope, next)?
                && (b.kind != K::Wander
                    || !flag(b, F::AvoidWater)
                    || !self
                        .map
                        .cell(next.x, next.y)
                        .is_some_and(|c| self.content.sinking_grounds.contains(&c.ground))));
        }
        if distance <= 0.00001 {
            if b.kind != K::Follow {
                self.xml.motions.remove(&step.token);
            }
            return Ok(false);
        }
        let next = point(pos, angle, distance);
        ensure!(
            next.x.is_finite() && next.y.is_finite(),
            "unrepresentable native motion endpoint x={} y={}",
            next.x,
            next.y
        );
        if b.kind != K::BackAndForth && !self.xml_npc_walkable(step.token.scope, next)? {
            return Ok(false);
        }
        let count = (distance / 0.1).ceil().max(1.);
        let max_count =
            (f64::from(self.map.width).hypot(f64::from(self.map.height)) / 0.1).ceil() + 1.;
        ensure!(
            count.is_finite() && count <= max_count && count < usize::MAX as f64,
            "native motion sample count unrepresentable count={count} map_bound={max_count}"
        );
        let samples = count as usize;
        let water = |p: Pos| {
            self.map
                .cell(p.x, p.y)
                .is_some_and(|c| self.content.sinking_grounds.contains(&c.ground))
        };
        let mut clear = true;
        for i in 1..=samples {
            let p = Pos {
                x: pos.x + (next.x - pos.x) * i as f32 / samples as f32,
                y: pos.y + (next.y - pos.y) * i as f32 / samples as f32,
            };
            if !self.xml_npc_walkable(step.token.scope, p)?
                || ((b.kind == K::Wander && flag(b, F::AvoidWater)) && water(p))
            {
                clear = false;
                break;
            }
        }
        if !clear {
            // Recovered behavior.cc:1625 reverses at a blocked endpoint without resetting the center.
            if b.kind == K::BackAndForth {
                m.direction = -m.direction;
                m.goal = point(m.origin, m.angle, n(b, F::Radius) * m.direction);
                self.xml.motions.insert(step.token, m);
                return Ok(true);
            }
            if b.kind == K::Orbit {
                m.direction = -m.direction;
                self.xml.motions.insert(step.token, m);
            } else if b.kind != K::Follow {
                self.xml.motions.remove(&step.token);
            }
            return Ok(false);
        }
        self.position_entity(id, next)?;
        self.entities
            .get_mut(&id)
            .expect("positioned XML actor")
            .last_step = Pos {
            x: next.x - pos.x,
            y: next.y - pos.y,
        };
        if b.kind == K::Wander {
            m.wander_remaining -= f64::from(pos.distance(next));
            if m.wander_remaining <= 0. {
                self.xml.motions.remove(&step.token);
                return Ok(true);
            }
        }
        self.xml.motions.insert(step.token, m);
        Ok(true)
    }
}
