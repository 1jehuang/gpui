//! Scene bookkeeping that lets a replay find a primitive by where it was emitted, and sorting that allocates nothing.

use crate::{
    MonochromeSprite, PaintOperation, PaintSurface, Path, PolychromeSprite, Primitive,
    PrimitiveKind, Quad, ScaledPixels, Scene, Shadow, SubpixelSprite, Underline,
};
use std::mem;

/// What a [`Scene`] keeps so that a replay can find last frame's primitives
/// after [`Scene::finish`] sorted them.
#[derive(Default)]
pub(crate) struct SceneOrdering {
    /// Where each primitive ended up once [`Scene::finish`] put them in
    /// drawing order, indexed by the position it was emitted at.
    ///
    /// A replay names primitives by where they were emitted, which is the only
    /// thing it can know; this is what turns that back into where they are.
    sorted_positions: SortedPositions,
    /// Room to gather each vector into while sorting, kept between frames so
    /// that a frame does not allocate megabytes to reorder what it drew.
    scratch: SceneScratch,
}

impl SceneOrdering {
    pub(crate) fn clear(&mut self) {
        self.sorted_positions.clear();
    }
}

/// The inverse of the permutation [`Scene::finish`] applies, one vector per
/// kind of primitive.
#[derive(Default)]
struct SortedPositions {
    shadows: Vec<u32>,
    quads: Vec<u32>,
    paths: Vec<u32>,
    underlines: Vec<u32>,
    monochrome_sprites: Vec<u32>,
    subpixel_sprites: Vec<u32>,
    polychrome_sprites: Vec<u32>,
    surfaces: Vec<u32>,
}

impl SortedPositions {
    fn clear(&mut self) {
        self.shadows.clear();
        self.quads.clear();
        self.paths.clear();
        self.underlines.clear();
        self.monochrome_sprites.clear();
        self.subpixel_sprites.clear();
        self.polychrome_sprites.clear();
        self.surfaces.clear();
    }
}

/// Buffers `Scene::finish` reorders through, kept so that sorting a scene
/// allocates nothing.
#[derive(Default)]
struct SceneScratch {
    order: Vec<u32>,
    shadows: Vec<Shadow>,
    quads: Vec<Quad>,
    paths: Vec<Path<ScaledPixels>>,
    underlines: Vec<Underline>,
    monochrome_sprites: Vec<MonochromeSprite>,
    subpixel_sprites: Vec<SubpixelSprite>,
    polychrome_sprites: Vec<PolychromeSprite>,
    surfaces: Vec<PaintSurface>,
}

/// Puts `items` in the order `key` gives, and records where each of them
/// ended up.
///
/// Sorting indices and then gathering once moves each item a single time,
/// where sorting the items themselves moves them as often as the sort needs
/// to compare them — and these items are 112 to 168 bytes each. The positions
/// fall out of the same pass, and are what lets a replay find a primitive it
/// only knows the emission order of.
fn sort_recording_positions<T: Clone, K: Ord>(
    items: &mut Vec<T>,
    order: &mut Vec<u32>,
    gathered: &mut Vec<T>,
    positions: &mut Vec<u32>,
    key: impl Fn(&T) -> K,
) {
    let count = items.len();
    positions.clear();
    if count == 0 {
        return;
    }

    order.clear();
    order.extend(0..count as u32);
    order.sort_unstable_by_key(|&index| key(&items[index as usize]));

    gathered.clear();
    gathered.extend(order.iter().map(|&index| items[index as usize].clone()));
    mem::swap(items, gathered);

    positions.resize(count, 0);
    for (destination, &source) in order.iter().enumerate() {
        positions[source as usize] = destination as u32;
    }
}

impl Scene {
    /// The operation recording `primitive`, which [`Scene::insert_primitive`]
    /// has just pushed: which vector it went into and where it was emitted.
    pub(crate) fn primitive_operation(&self, primitive: &Primitive) -> PaintOperation {
        let (kind, len) = match primitive {
            Primitive::Shadow(_) => (PrimitiveKind::Shadow, self.shadows.len()),
            Primitive::Quad(_) => (PrimitiveKind::Quad, self.quads.len()),
            Primitive::Path(_) => (PrimitiveKind::Path, self.paths.len()),
            Primitive::Underline(_) => (PrimitiveKind::Underline, self.underlines.len()),
            Primitive::MonochromeSprite(_) => (
                PrimitiveKind::MonochromeSprite,
                self.monochrome_sprites.len(),
            ),
            Primitive::SubpixelSprite(_) => {
                (PrimitiveKind::SubpixelSprite, self.subpixel_sprites.len())
            }
            Primitive::PolychromeSprite(_) => (
                PrimitiveKind::PolychromeSprite,
                self.polychrome_sprites.len(),
            ),
            Primitive::Surface(_) => (PrimitiveKind::Surface, self.surfaces.len()),
        };
        PaintOperation::Primitive(kind, len as u32 - 1)
    }

    /// Inserts again the primitive `prev_scene` emitted in position
    /// `emitted_at`.
    pub(crate) fn replay_primitive(
        &mut self,
        prev_scene: &Scene,
        kind: PrimitiveKind,
        emitted_at: u32,
    ) {
        if let Some(primitive) = prev_scene.primitive_emitted_at(kind, emitted_at) {
            self.insert_primitive(primitive);
        }
    }

    /// The primitive this scene emitted in position `emitted_at`, wherever
    /// [`Scene::finish`] has since moved it to.
    fn primitive_emitted_at(&self, kind: PrimitiveKind, emitted_at: u32) -> Option<Primitive> {
        fn at<T: Clone>(items: &[T], positions: &[u32], emitted_at: u32) -> Option<T> {
            let index = *positions.get(emitted_at as usize)? as usize;
            items.get(index).cloned()
        }
        let positions = &self.ordering.sorted_positions;
        match kind {
            PrimitiveKind::Shadow => {
                at(&self.shadows, &positions.shadows, emitted_at).map(Primitive::Shadow)
            }
            PrimitiveKind::Quad => {
                at(&self.quads, &positions.quads, emitted_at).map(Primitive::Quad)
            }
            PrimitiveKind::Path => {
                at(&self.paths, &positions.paths, emitted_at).map(Primitive::Path)
            }
            PrimitiveKind::Underline => {
                at(&self.underlines, &positions.underlines, emitted_at).map(Primitive::Underline)
            }
            PrimitiveKind::MonochromeSprite => at(
                &self.monochrome_sprites,
                &positions.monochrome_sprites,
                emitted_at,
            )
            .map(Primitive::MonochromeSprite),
            PrimitiveKind::SubpixelSprite => at(
                &self.subpixel_sprites,
                &positions.subpixel_sprites,
                emitted_at,
            )
            .map(Primitive::SubpixelSprite),
            PrimitiveKind::PolychromeSprite => at(
                &self.polychrome_sprites,
                &positions.polychrome_sprites,
                emitted_at,
            )
            .map(Primitive::PolychromeSprite),
            PrimitiveKind::Surface => {
                at(&self.surfaces, &positions.surfaces, emitted_at).map(Primitive::Surface)
            }
        }
    }

    /// What [`Scene::finish`] does: puts every primitive in drawing order,
    /// recording where each one went.
    pub(crate) fn sort_in_drawing_order(&mut self) {
        let scratch = &mut self.ordering.scratch;
        let positions = &mut self.ordering.sorted_positions;
        macro_rules! sort {
            ($field:ident, $key:expr) => {
                sort_recording_positions(
                    &mut self.$field,
                    &mut scratch.order,
                    &mut scratch.$field,
                    &mut positions.$field,
                    $key,
                )
            };
        }
        // Sprites of one order are grouped by texture: a batch draws from one
        // texture, and tile ids, numbered per texture, would interleave them.
        sort!(shadows, |shadow: &Shadow| shadow.order);
        sort!(quads, |quad: &Quad| quad.order);
        sort!(paths, |path: &Path<ScaledPixels>| path.order);
        sort!(underlines, |underline: &Underline| underline.order);
        sort!(monochrome_sprites, |sprite: &MonochromeSprite| (
            sprite.order,
            sprite.tile.texture_id.index,
            sprite.tile.tile_id
        ));
        sort!(subpixel_sprites, |sprite: &SubpixelSprite| (
            sprite.order,
            sprite.tile.texture_id.index,
            sprite.tile.tile_id
        ));
        sort!(polychrome_sprites, |sprite: &PolychromeSprite| (
            sprite.order,
            sprite.tile.texture_id.index,
            sprite.tile.tile_id
        ));
        sort!(surfaces, |surface: &PaintSurface| surface.order);
    }

    /// Forgets the orderings recorded for replaying, so the next frame orders
    /// every primitive from scratch.
    #[cfg(test)]
    pub(crate) fn forget_orderings(&mut self) {
        self.primitive_bounds.forget();
    }

    /// Everything this finished scene draws, in drawing order, as text two
    /// scenes can be compared by: each primitive with its bounds, clip, colours
    /// and ordering, and each layer's bounds. Atlas tiles are left out, since
    /// two windows need not place the same glyph in the same tile.
    #[cfg(test)]
    pub(crate) fn describe(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for operation in &self.paint_operations {
            match operation {
                PaintOperation::StartLayer(bounds) => lines.push(format!("layer {bounds:?}")),
                PaintOperation::EndLayer => lines.push("end layer".into()),
                PaintOperation::Primitive(..) => {}
            }
        }
        lines.extend(self.shadows.iter().map(|shadow| format!("{shadow:?}")));
        lines.extend(self.quads.iter().map(|quad| format!("{quad:?}")));
        lines.extend(
            self.underlines
                .iter()
                .map(|underline| format!("{underline:?}")),
        );
        lines.extend(self.monochrome_sprites.iter().map(|sprite| {
            format!(
                "monochrome sprite {} {:?} {:?} {:?}",
                sprite.order, sprite.bounds, sprite.content_mask, sprite.color
            )
        }));
        lines.extend(self.subpixel_sprites.iter().map(|sprite| {
            format!(
                "subpixel sprite {} {:?} {:?} {:?}",
                sprite.order, sprite.bounds, sprite.content_mask, sprite.color
            )
        }));
        lines.extend(self.polychrome_sprites.iter().map(|sprite| {
            format!(
                "polychrome sprite {} {:?} {:?}",
                sprite.order, sprite.bounds, sprite.content_mask
            )
        }));
        lines.extend(
            self.paths
                .iter()
                .map(|path| format!("path {} {:?}", path.order, path.bounds)),
        );
        lines
    }
}
