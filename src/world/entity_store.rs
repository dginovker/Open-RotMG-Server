use super::*;
use std::ops::Deref;

#[derive(Default)]
pub struct Entities {
    values: BTreeMap<i32, Entity>,
    pub(super) promotions: u64,
    shared: usize,
}

impl Deref for Entities {
    type Target = BTreeMap<i32, Entity>;
    fn deref(&self) -> &Self::Target {
        &self.values
    }
}

impl Entities {
    pub fn insert(&mut self, id: i32, entity: Entity) -> Option<Entity> {
        self.shared += usize::from(matches!(entity.state, EntityState::Shared(_)));
        let old = self.values.insert(id, entity);
        self.shared -= usize::from(
            old.as_ref()
                .is_some_and(|e| matches!(e.state, EntityState::Shared(_))),
        );
        old
    }
    pub fn get_mut(&mut self, id: &i32) -> Option<&mut Entity> {
        let entity = self.values.get_mut(id)?;
        if entity.promote() {
            self.promotions += 1;
            self.shared -= 1;
            tracing::debug!(
                entity = id,
                kind = entity.kind,
                "scenery promoted for mutation"
            );
        }
        Some(entity)
    }
    pub fn remove(&mut self, id: &i32) -> Option<Entity> {
        let entity = self.values.remove(id)?;
        self.shared -= usize::from(matches!(entity.state, EntityState::Shared(_)));
        Some(entity)
    }
    // inspect receives &&mut Entity, which cannot promote the selected entity.
    #[allow(clippy::manual_inspect)]
    pub fn matching_mut(
        &mut self,
        mut predicate: impl FnMut(&Entity) -> bool,
    ) -> impl Iterator<Item = &mut Entity> {
        let promotions = &mut self.promotions;
        let shared = &mut self.shared;
        self.values
            .values_mut()
            .filter(move |e| predicate(e))
            .map(move |e| {
                if e.promote() {
                    *promotions += 1;
                    *shared -= 1;
                }
                e
            })
    }
    pub(super) fn shared(&self) -> usize {
        self.shared
    }
}

#[derive(Clone, Debug)]
pub(super) enum EntityState {
    Owned(Box<EntityData>),
    Shared(Arc<EntityData>),
}

impl Entity {
    pub(super) fn share(
        &mut self,
        templates: &mut BTreeMap<(u16, String), Arc<EntityData>>,
        config: &str,
    ) {
        let key = (self.kind, config.to_owned());
        let data = templates
            .entry(key)
            .or_insert_with(|| Arc::new((**self).clone()));
        self.state = EntityState::Shared(data.clone());
    }
    fn promote(&mut self) -> bool {
        if let EntityState::Shared(data) = &self.state {
            self.state = EntityState::Owned(Box::new((**data).clone()));
            true
        } else {
            false
        }
    }
}

impl std::ops::Deref for Entity {
    type Target = EntityData;
    fn deref(&self) -> &EntityData {
        match &self.state {
            EntityState::Owned(data) => data,
            EntityState::Shared(data) => data,
        }
    }
}
impl std::ops::DerefMut for Entity {
    fn deref_mut(&mut self) -> &mut EntityData {
        match &mut self.state {
            EntityState::Owned(data) => data,
            EntityState::Shared(_) => panic!("shared scenery mutation must use Entities::get_mut"),
        }
    }
}
