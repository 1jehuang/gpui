//! Tests of how a scene orders what it draws.

use crate::{
    AtlasTextureId, AtlasTextureKind, AtlasTile, Bounds, ContentMask, DevicePixels, Hsla,
    MonochromeSprite, PrimitiveBatch, ScaledPixels, Scene, TileId, TransformationMatrix, point,
    size,
};

fn glyph(x: f32, texture: u32, tile: u32) -> MonochromeSprite {
    let bounds = Bounds::new(
        point(ScaledPixels(x), ScaledPixels(0.)),
        size(ScaledPixels(8.), ScaledPixels(8.)),
    );
    MonochromeSprite {
        order: 0,
        pad: 0,
        bounds,
        content_mask: ContentMask { bounds },
        color: Hsla::default(),
        tile: AtlasTile {
            texture_id: AtlasTextureId {
                index: texture,
                kind: AtlasTextureKind::Monochrome,
            },
            tile_id: TileId(tile),
            padding: 0,
            bounds: Bounds::new(
                point(DevicePixels(0), DevicePixels(0)),
                size(DevicePixels(8), DevicePixels(8)),
            ),
        },
        transformation: TransformationMatrix::unit(),
    }
}

/// Glyphs that do not overlap share an order however many atlas textures
/// they come from, and are drawn in one batch per texture, not one per run
/// of glyphs from the same texture.
#[test]
fn glyphs_of_one_order_are_batched_by_texture() {
    let mut scene = Scene::default();
    for i in 0..20 {
        // Tile ids are numbered per texture, so both textures have each.
        scene.insert_primitive(glyph(i as f32 * 10., i % 2, i / 2));
    }
    scene.finish();
    let batches = scene
        .batches()
        .map(|batch| match batch {
            PrimitiveBatch::MonochromeSprites { texture_id, range } => {
                (texture_id.index, range.len())
            }
            _ => panic!("only glyphs were drawn"),
        })
        .collect::<Vec<_>>();
    assert_eq!(batches, vec![(0, 10), (1, 10)]);
}
