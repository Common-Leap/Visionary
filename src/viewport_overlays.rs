//! Where the 3D preview's overlays sit: the stage picker, the Effects page's view switch, the
//! viewport toolbar and the orientation widget. They are placed together, from the sizes they
//! were drawn at last, so none covers another at any view width.

use egui::{pos2, vec2, Pos2, Rect, Vec2};

/// Distance from the view's edges.
pub const MARGIN: f32 = 8.0;
/// Distance between two overlays.
const GAP: f32 = 6.0;

/// The overlays' sizes when last drawn; `None` for one that is not shown.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Sizes {
    pub stage: Option<Vec2>,
    pub switch: Option<Vec2>,
    pub toolbar: Option<Vec2>,
}

/// Where each overlay goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Places {
    pub stage: Pos2,
    pub switch: Pos2,
    pub toolbar: Pos2,
    /// `None` when the view has no room for the widget clear of the others.
    pub widget: Option<Rect>,
}

/// Place the overlays in `view`: the stage picker in the top left corner, the switch beside it
/// (under it in a narrow view), the toolbar on the row under both, and the widget, `widget`
/// large, in the top right corner under whatever reaches that far.
pub fn place(view: Rect, sizes: Sizes, widget: Option<Vec2>) -> Places {
    let rect = |at: Pos2, size: Option<Vec2>| size.map(|size| Rect::from_min_size(at, size));
    let left = view.left() + MARGIN;
    let stage = pos2(left, view.top() + MARGIN);
    let mut taken: Vec<Rect> = rect(stage, sizes.stage).into_iter().collect();
    let under = |taken: &[Rect]| {
        taken
            .iter()
            .map(|rect| rect.bottom() + GAP)
            .fold(view.top() + MARGIN, f32::max)
    };

    let beside = taken
        .last()
        .map_or(left, |stage| stage.right() + GAP)
        .max(left + crate::app::STAGE_PICKER_WIDTH + 30.0);
    let fits_beside = sizes
        .switch
        .is_none_or(|size| beside + size.x + MARGIN <= view.right());
    let switch = if fits_beside {
        pos2(beside, view.top() + MARGIN)
    } else {
        pos2(left, under(&taken).max(view.top() + 40.0))
    };
    taken.extend(rect(switch, sizes.switch));

    let toolbar = pos2(left, under(&taken));
    taken.extend(rect(toolbar, sizes.toolbar));

    let widget = widget.and_then(|size| {
        let x = view.right() - MARGIN - size.x;
        let top = taken
            .iter()
            .filter(|rect| rect.right() + MARGIN > x)
            .map(|rect| rect.bottom() + MARGIN)
            .fold(view.top() + MARGIN, f32::max);
        let rect = Rect::from_min_size(pos2(x, top), size);
        view.shrink2(vec2(MARGIN, MARGIN))
            .contains_rect(rect)
            .then_some(rect)
    });
    Places {
        stage,
        switch,
        toolbar,
        widget,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No overlay covers another, whatever the view's size, with every mix of them shown.
    #[test]
    fn overlays_never_cover_one_another() {
        let stage = vec2(180.0, 24.0);
        let mut widgets_shown = 0;
        for width in 240..=1400 {
            for height in [150.0, 300.0, 700.0] {
                let view = Rect::from_min_size(pos2(350.0, 75.0), vec2(width as f32, height));
                let room = view.width() - 2.0 * MARGIN;
                for switch in [None, Some(170.0), Some(280.0), Some(420.0)] {
                    for toolbar in [None, Some(60.0), Some(500.0), Some(2000.0)] {
                        for widget in [None, Some(86.0), Some(136.0)] {
                            let sizes = Sizes {
                                stage: Some(stage),
                                switch: switch.map(|w: f32| vec2(w.min(room), 34.0)),
                                // A toolbar wider than the view wraps onto more rows.
                                toolbar: toolbar
                                    .map(|w: f32| vec2(w.min(room), 28.0 * (w / room).ceil())),
                            };
                            let at = place(view, sizes, widget.map(Vec2::splat));
                            let rects: Vec<Rect> = [
                                (Some(at.stage), sizes.stage),
                                (Some(at.switch), sizes.switch),
                                (Some(at.toolbar), sizes.toolbar),
                                (at.widget.map(|r| r.min), at.widget.map(|r| r.size())),
                            ]
                            .into_iter()
                            .filter_map(|(at, size)| Some(Rect::from_min_size(at?, size?)))
                            .collect();
                            widgets_shown += usize::from(at.widget.is_some());
                            for (i, a) in rects.iter().enumerate() {
                                assert!(
                                    a.left() >= view.left() && a.right() <= view.right(),
                                    "{width}x{height}: {a:?} leaves {view:?}"
                                );
                                for b in &rects[i + 1..] {
                                    assert!(
                                        !a.intersect(*b).is_positive(),
                                        "{width}x{height} {sizes:?}: {a:?} covers {b:?}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        // The widget gave way only where there was no room for it.
        assert!(widgets_shown > 10_000, "{widgets_shown}");
    }

    /// The stage picker and the switch keep the places they had before there was a toolbar.
    #[test]
    fn the_stage_picker_and_switch_keep_their_places() {
        let sizes = Sizes {
            stage: Some(vec2(150.0, 21.0)),
            switch: Some(vec2(280.0, 34.0)),
            toolbar: Some(vec2(120.0, 28.0)),
        };
        let at = |width: f32| {
            let view = Rect::from_min_size(Pos2::ZERO, vec2(width, 600.0));
            place(view, sizes, Some(Vec2::splat(86.0)))
        };
        assert_eq!(at(900.0).stage, pos2(8.0, 8.0));
        assert_eq!(at(900.0).switch, pos2(188.0, 8.0));
        assert_eq!(at(400.0).switch, pos2(8.0, 40.0));
        // A wide view's widget is in the corner; a narrow one's is under the row it would cover.
        assert_eq!(at(900.0).widget.unwrap().top(), 8.0);
        assert!(at(500.0).widget.unwrap().top() > 42.0);
    }
}
