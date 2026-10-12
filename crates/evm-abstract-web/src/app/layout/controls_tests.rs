use egui::{Context, Event, Key, Modifiers, Vec2};
use egui_tiles::{Container, Tile, TileId, Tree};

use super::{
    Pane,
    tests::{active_tree, frame, settled},
};
use crate::{Workspace, controls::Density, palette, tests::ready};

fn tile(workspace: &Workspace, width: f32, pane: Pane) -> TileId {
    active_tree(workspace, width)
        .tiles
        .iter()
        .find_map(|(id, tile)| match tile {
            Tile::Pane(value) if *value == pane => Some(*id),
            _ => None,
        })
        .unwrap()
}

fn key(key: Key) -> Vec<Event> {
    [true, false]
        .into_iter()
        .map(|pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        })
        .collect()
}

fn selected(tree: &Tree<Pane>, id: TileId) -> bool {
    tree.tiles.iter().any(|(_, tile)| {
        matches!(tile,
            Tile::Container(Container::Tabs(tabs)) if tabs.active == Some(id)
        )
    })
}

#[test]
fn workspace_tile_tabs_share_control_geometry_and_keep_library_focus_and_selection() {
    for size in [Vec2::new(390.0, 800.0), Vec2::new(960.0, 800.0)] {
        let ctx = Context::default();
        palette::configure(&ctx);
        let mut workspace = ready();
        settled(&ctx, &mut workspace, size);
        for pane in [Pane::Disassembly, Pane::Ssa] {
            let tree = active_tree(&workspace, size.x);
            let tile = tile(&workspace, size.x, pane);
            let id = tile.egui_id(tree.id());
            let before = ctx
                .read_response(id)
                .expect("the real tile ID must own the tab response");
            assert_eq!(before.rect.height(), Density::Regular.height());
            ctx.memory_mut(|memory| memory.request_focus(id));
            let output = frame(&ctx, &mut workspace, size, vec![]);
            let focused = ctx.read_response(id).unwrap();
            assert!(focused.has_focus());
            assert_eq!(focused.rect, before.rect);
            assert!(
                output.shapes.iter().any(|shape| matches!(&shape.shape,
                    egui::Shape::Rect(rect)
                        if rect.rect == focused.rect.shrink(2.0)
                            && rect.stroke == egui::Stroke::new(2.0, palette::BLUE)
                )),
                "the tile tab must paint the shared keyboard focus ring"
            );
            frame(&ctx, &mut workspace, size, key(Key::Enter));
            let output = settled(&ctx, &mut workspace, size);
            let tree = active_tree(&workspace, size.x);
            assert!(
                selected(tree, tile),
                "keyboard activation must change the real active tile"
            );
            let tab = ctx.read_response(id).unwrap();
            assert!(tab.has_focus());
            assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
                egui::Shape::Rect(rect)
                    if rect.rect == tab.rect
                        && rect.fill == palette::SELECTED
                        && rect.corner_radius == egui::CornerRadius::same(4)
            )));
            let other = tile_for_other_source(&workspace, size.x, pane);
            assert!(!selected(tree, other));
            frame(&ctx, &mut workspace, size, key(Key::Space));
            settled(&ctx, &mut workspace, size);
            assert!(
                selected(active_tree(&workspace, size.x), tile),
                "activating the selected tab cannot deselect it"
            );
        }
        if size.x < 720.0 {
            let tree = active_tree(&workspace, size.x);
            let graph = tile(&workspace, size.x, Pane::Graph);
            let id = graph.egui_id(tree.id());
            ctx.memory_mut(|memory| memory.request_focus(id));
            frame(&ctx, &mut workspace, size, vec![]);
            frame(&ctx, &mut workspace, size, key(Key::Enter));
            settled(&ctx, &mut workspace, size);
            assert!(selected(active_tree(&workspace, size.x), graph));
        }
    }
}

fn tile_for_other_source(workspace: &Workspace, width: f32, pane: Pane) -> TileId {
    tile(
        workspace,
        width,
        if pane == Pane::Ssa {
            Pane::Disassembly
        } else {
            Pane::Ssa
        },
    )
}
