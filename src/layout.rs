use std::{collections::BTreeMap, fs, io, path::PathBuf};

use ratatui::{
    layout::{Direction, Rect},
    widgets::Borders,
};

use crate::{pane::Pane, ui::AGENT_PRESETS};

#[derive(Clone, Copy, Default)]
pub(crate) struct ExposedSides {
    pub(crate) top: bool,
    pub(crate) bottom: bool,
    pub(crate) left: bool,
    pub(crate) right: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum SplitSide {
    Top,
    Bottom,
    Left,
    Right,
}

pub(crate) struct PersistedLayout {
    pub(crate) layout: Node,
    pub(crate) focused: usize,
    pub(crate) default_agent_index: usize,
    pub(crate) titles: BTreeMap<usize, String>,
    pub(crate) commands: BTreeMap<usize, String>,
    /// Per-pane captured resume command (e.g. "codex resume <id>"). When
    /// present at startup, the pane is launched by running this verbatim via
    /// `/bin/sh -c` rather than the canonical agent command.
    pub(crate) resume_commands: BTreeMap<usize, String>,
}

pub(crate) struct Placement {
    pub(crate) pane_id: usize,
    pub(crate) area: Rect,
    pub(crate) exposed: ExposedSides,
}

pub(crate) struct DebugPlacement {
    pub(crate) pane_id: usize,
    pub(crate) container_area: Rect,
    pub(crate) pane_area: Rect,
}

pub(crate) struct DebugContainer {
    pub(crate) area: Rect,
    pub(crate) divider_area: Rect,
}

pub(crate) struct ResizeBoundary {
    pub(crate) direction: Direction,
    pub(crate) first_area: Rect,
    pub(crate) second_area: Rect,
    pub(crate) divider_area: Option<Rect>,
    pub(crate) first_pane_ids: Vec<usize>,
    pub(crate) second_pane_ids: Vec<usize>,
    pub(crate) depth: u8,
}

struct SplitChunks {
    first: Rect,
    divider: Rect,
    second: Rect,
}

const RATIO_SCALE: u16 = 10_000;
const RATIO_HALF: u16 = RATIO_SCALE / 2;

pub(crate) const PANE_INNER_MARGIN: u16 = 1;
// Single row of title text directly under the top border. PANE_INNER_MARGIN
// below already separates it from the pane contents.
pub(crate) const PANE_TITLE_BAR_HEIGHT: u16 = 1;

pub(crate) fn pane_borders(_exposed: ExposedSides) -> Borders {
    // Every pane owns all four borders. Adjacent panes therefore have distinct
    // edge cells instead of visually sharing one edge.
    Borders::ALL
}

pub(crate) fn pane_title_y(area: Rect) -> u16 {
    // Title text sits on the first row of the title bar (just under the top
    // border).
    area.y.saturating_add(1)
}

pub(crate) fn pane_title_bar_area(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: PANE_TITLE_BAR_HEIGHT.min(area.height.saturating_sub(2)),
    }
}

pub(crate) fn pane_inner_area(area: Rect, _exposed: ExposedSides) -> Rect {
    let inset = 1 + PANE_INNER_MARGIN;
    let top_chrome = 1 + PANE_TITLE_BAR_HEIGHT + PANE_INNER_MARGIN;
    Rect {
        x: area.x.saturating_add(inset),
        y: area.y.saturating_add(top_chrome),
        width: area.width.saturating_sub(inset.saturating_mul(2)),
        height: area
            .height
            .saturating_sub(top_chrome + 1 + PANE_INNER_MARGIN),
    }
}

pub(crate) fn pane_title_hit_area(area: Rect, title: &str) -> Option<Rect> {
    let title_y = pane_title_y(area);
    if area.width <= 2 || area.height <= PANE_TITLE_BAR_HEIGHT {
        return None;
    }

    // The title centers over the full title-bar width (between the left and
    // right borders). Maximize/close take priority in click handling, so an
    // overlap with the controls is harmless.
    let bar_width = area.width.saturating_sub(2);
    if bar_width == 0 {
        return None;
    }

    let text_width = (title.chars().count() as u16).min(bar_width);
    let offset = bar_width.saturating_sub(text_width) / 2;
    Some(Rect {
        x: area.x.saturating_add(1).saturating_add(offset),
        y: title_y,
        width: text_width,
        height: 1,
    })
}

impl Placement {
    pub(crate) fn title_hit(&self, title: &str, _focused: bool, x: u16, y: u16) -> bool {
        pane_title_hit_area(self.area, title).is_some_and(|area| contains(area, x, y))
    }

    pub(crate) fn maximize_hit(&self, x: u16, y: u16) -> bool {
        if self.area.width < 9 || self.area.height <= PANE_TITLE_BAR_HEIGHT {
            return false;
        }

        contains(
            Rect {
                x: self.area.right().saturating_sub(8),
                y: pane_title_y(self.area),
                width: 3,
                height: 1,
            },
            x,
            y,
        )
    }

    pub(crate) fn close_hit(&self, x: u16, y: u16) -> bool {
        if self.area.width < 6 || self.area.height <= PANE_TITLE_BAR_HEIGHT {
            return false;
        }

        contains(
            Rect {
                x: self.area.right().saturating_sub(4),
                y: pane_title_y(self.area),
                width: 3,
                height: 1,
            },
            x,
            y,
        )
    }
}

#[derive(Clone)]
pub(crate) enum Node {
    Leaf {
        pane_id: usize,
    },
    Split {
        direction: Direction,
        ratio: u16,
        first: Box<Node>,
        second: Box<Node>,
    },
}

impl Node {
    pub(crate) fn collect(&self, area: Rect, exposed: ExposedSides, out: &mut Vec<Placement>) {
        match self {
            Self::Leaf { pane_id } => out.push(Placement {
                pane_id: *pane_id,
                area,
                exposed,
            }),
            Self::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let first_ratio = ratio_basis_points(*ratio);
                let chunks = split_area_by_ratio(area, *direction, first_ratio);

                let mut first_exposed = exposed;
                let mut second_exposed = exposed;
                match direction {
                    Direction::Vertical => {
                        first_exposed.bottom = false;
                        second_exposed.top = false;
                    }
                    Direction::Horizontal => {
                        first_exposed.right = false;
                        second_exposed.left = false;
                    }
                }

                first.collect(chunks.first, first_exposed, out);
                second.collect(chunks.second, second_exposed, out);
            }
        }
    }

    pub(crate) fn placement_at(
        &self,
        area: Rect,
        exposed: ExposedSides,
        x: u16,
        y: u16,
    ) -> Option<Placement> {
        if !contains(area, x, y) {
            return None;
        }

        match self {
            Self::Leaf { pane_id } => Some(Placement {
                pane_id: *pane_id,
                area,
                exposed,
            }),
            Self::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let first_ratio = ratio_basis_points(*ratio);
                let chunks = split_area_by_ratio(area, *direction, first_ratio);

                let mut first_exposed = exposed;
                let mut second_exposed = exposed;
                match direction {
                    Direction::Vertical => {
                        first_exposed.bottom = false;
                        second_exposed.top = false;
                    }
                    Direction::Horizontal => {
                        first_exposed.right = false;
                        second_exposed.left = false;
                    }
                }

                if contains(chunks.first, x, y) {
                    first.placement_at(chunks.first, first_exposed, x, y)
                } else if contains(chunks.second, x, y) {
                    second.placement_at(chunks.second, second_exposed, x, y)
                } else {
                    None
                }
            }
        }
    }

    pub(crate) fn collect_debug_areas(
        &self,
        area: Rect,
        containers: &mut Vec<DebugContainer>,
        placements: &mut Vec<DebugPlacement>,
    ) {
        match self {
            Self::Leaf { pane_id } => {
                placements.push(DebugPlacement {
                    pane_id: *pane_id,
                    container_area: area,
                    pane_area: area,
                });
            }
            Self::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let first_ratio = ratio_basis_points(*ratio);
                let Some(chunks) =
                    split_debug_container_with_divider(area, *direction, first_ratio)
                else {
                    return;
                };
                containers.push(DebugContainer {
                    area,
                    divider_area: chunks.divider,
                });

                first.collect_debug_areas(chunks.first, containers, placements);
                second.collect_debug_areas(chunks.second, containers, placements);
            }
        }
    }

    pub(crate) fn collect_debug_resize_boundaries(
        &self,
        area: Rect,
        depth: u8,
        out: &mut Vec<ResizeBoundary>,
    ) {
        match self {
            Self::Leaf { .. } => {}
            Self::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let first_ratio = ratio_basis_points(*ratio);
                let Some(chunks) =
                    split_debug_container_with_divider(area, *direction, first_ratio)
                else {
                    return;
                };
                let mut first_pane_ids = Vec::new();
                let mut second_pane_ids = Vec::new();
                first.collect_leaf_ids(&mut first_pane_ids);
                second.collect_leaf_ids(&mut second_pane_ids);

                out.push(ResizeBoundary {
                    direction: *direction,
                    first_area: chunks.first,
                    second_area: chunks.second,
                    divider_area: Some(chunks.divider),
                    first_pane_ids,
                    second_pane_ids,
                    depth,
                });

                first.collect_debug_resize_boundaries(chunks.first, depth.saturating_add(1), out);
                second.collect_debug_resize_boundaries(chunks.second, depth.saturating_add(1), out);
            }
        }
    }

    pub(crate) fn collect_resize_boundaries(
        &self,
        area: Rect,
        depth: u8,
        out: &mut Vec<ResizeBoundary>,
    ) {
        match self {
            Self::Leaf { .. } => {}
            Self::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let first_ratio = ratio_basis_points(*ratio);
                let chunks = split_area_by_ratio(area, *direction, first_ratio);

                let mut first_pane_ids = Vec::new();
                let mut second_pane_ids = Vec::new();
                first.collect_leaf_ids(&mut first_pane_ids);
                second.collect_leaf_ids(&mut second_pane_ids);

                out.push(ResizeBoundary {
                    direction: *direction,
                    first_area: chunks.first,
                    second_area: chunks.second,
                    divider_area: Some(chunks.divider),
                    first_pane_ids,
                    second_pane_ids,
                    depth,
                });

                first.collect_resize_boundaries(chunks.first, depth.saturating_add(1), out);
                second.collect_resize_boundaries(chunks.second, depth.saturating_add(1), out);
            }
        }
    }

    pub(crate) fn split_leaf(&mut self, target_id: usize, side: SplitSide, new_id: usize) -> bool {
        match self {
            Self::Leaf { pane_id } if *pane_id == target_id => {
                let old_id = *pane_id;
                *self = match side {
                    SplitSide::Top => Self::Split {
                        direction: Direction::Vertical,
                        ratio: RATIO_HALF,
                        first: Box::new(Self::Leaf { pane_id: new_id }),
                        second: Box::new(Self::Leaf { pane_id: old_id }),
                    },
                    SplitSide::Bottom => Self::Split {
                        direction: Direction::Vertical,
                        ratio: RATIO_HALF,
                        first: Box::new(Self::Leaf { pane_id: old_id }),
                        second: Box::new(Self::Leaf { pane_id: new_id }),
                    },
                    SplitSide::Left => Self::Split {
                        direction: Direction::Horizontal,
                        ratio: RATIO_HALF,
                        first: Box::new(Self::Leaf { pane_id: new_id }),
                        second: Box::new(Self::Leaf { pane_id: old_id }),
                    },
                    SplitSide::Right => Self::Split {
                        direction: Direction::Horizontal,
                        ratio: RATIO_HALF,
                        first: Box::new(Self::Leaf { pane_id: old_id }),
                        second: Box::new(Self::Leaf { pane_id: new_id }),
                    },
                };
                true
            }
            Self::Leaf { .. } => false,
            Self::Split { first, second, .. } => {
                first.split_leaf(target_id, side, new_id)
                    || second.split_leaf(target_id, side, new_id)
            }
        }
    }

    pub(crate) fn resize_leaf_edge(
        &mut self,
        target_id: usize,
        side: SplitSide,
        amount: u16,
        area: Rect,
        exposed: ExposedSides,
    ) -> bool {
        match self {
            Self::Leaf { .. } => false,
            Self::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let first_has = first.contains_pane_id(target_id);
                let second_has = second.contains_pane_id(target_id);

                let first_ratio = ratio_basis_points(*ratio);
                let chunks = split_area_by_ratio(area, *direction, first_ratio);

                let mut first_exposed = exposed;
                let mut second_exposed = exposed;
                match direction {
                    Direction::Vertical => {
                        first_exposed.bottom = false;
                        second_exposed.top = false;
                    }
                    Direction::Horizontal => {
                        first_exposed.right = false;
                        second_exposed.left = false;
                    }
                }

                if first_has
                    && first.resize_leaf_edge(target_id, side, amount, chunks.first, first_exposed)
                {
                    return true;
                }

                if second_has
                    && second.resize_leaf_edge(
                        target_id,
                        side,
                        amount,
                        chunks.second,
                        second_exposed,
                    )
                {
                    return true;
                }

                let (current, max_first, total, valid) = match direction {
                    Direction::Horizontal if area.width > 1 => (
                        i32::from(chunks.first.width),
                        i32::from(area.width.saturating_sub(1)),
                        i32::from(area.width),
                        true,
                    ),
                    Direction::Vertical if area.height > 1 => (
                        i32::from(chunks.first.height),
                        i32::from(area.height.saturating_sub(1)),
                        i32::from(area.height),
                        true,
                    ),
                    _ => (0, 0, 0, false),
                };

                if !valid {
                    return false;
                }

                let next = match (direction, side) {
                    (Direction::Horizontal, SplitSide::Left) if first_has || second_has => {
                        current - i32::from(amount)
                    }
                    (Direction::Horizontal, SplitSide::Right) if first_has || second_has => {
                        current + i32::from(amount)
                    }
                    (Direction::Vertical, SplitSide::Top) if first_has || second_has => {
                        current - i32::from(amount)
                    }
                    (Direction::Vertical, SplitSide::Bottom) if first_has || second_has => {
                        current + i32::from(amount)
                    }
                    _ => return false,
                }
                .clamp(1, max_first);

                *ratio = ratio_for_first_rendered_size(next, total);
                true
            }
        }
    }

    pub(crate) fn resize_between(
        &mut self,
        pane_a: usize,
        pane_b: usize,
        side: SplitSide,
        amount: u16,
        area: Rect,
    ) -> bool {
        match self {
            Self::Leaf { .. } => false,
            Self::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let first_ratio = ratio_basis_points(*ratio);
                let chunks = split_area_by_ratio(area, *direction, first_ratio);

                let a_in_first = first.contains_pane_id(pane_a);
                let b_in_first = first.contains_pane_id(pane_b);
                let a_in_second = second.contains_pane_id(pane_a);
                let b_in_second = second.contains_pane_id(pane_b);

                if (a_in_first && b_in_first)
                    && first.resize_between(pane_a, pane_b, side, amount, chunks.first)
                {
                    return true;
                }
                if (a_in_second && b_in_second)
                    && second.resize_between(pane_a, pane_b, side, amount, chunks.second)
                {
                    return true;
                }

                let opposite_sides = (a_in_first && b_in_second) || (a_in_second && b_in_first);
                if !opposite_sides {
                    return false;
                }

                let (current, max_first, total, allowed) = match (*direction, side) {
                    (Direction::Horizontal, SplitSide::Left | SplitSide::Right)
                        if area.width > 1 =>
                    {
                        (
                            i32::from(chunks.first.width),
                            i32::from(area.width.saturating_sub(1)),
                            i32::from(area.width),
                            true,
                        )
                    }
                    (Direction::Vertical, SplitSide::Top | SplitSide::Bottom)
                        if area.height > 1 =>
                    {
                        (
                            i32::from(chunks.first.height),
                            i32::from(area.height.saturating_sub(1)),
                            i32::from(area.height),
                            true,
                        )
                    }
                    _ => (0, 0, 0, false),
                };

                if !allowed {
                    return false;
                }

                let signed = match side {
                    SplitSide::Left | SplitSide::Top => -(i32::from(amount)),
                    SplitSide::Right | SplitSide::Bottom => i32::from(amount),
                };
                let next = (current + signed).clamp(1, max_first);
                *ratio = ratio_for_first_rendered_size(next, total);
                true
            }
        }
    }

    pub(crate) fn resize_between_debug(
        &mut self,
        pane_a: usize,
        pane_b: usize,
        side: SplitSide,
        amount: u16,
        area: Rect,
    ) -> bool {
        match self {
            Self::Leaf { .. } => false,
            Self::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let inner = inset_rect(area, 1);
                if inner.width == 0 || inner.height == 0 {
                    return false;
                }

                let first_ratio = ratio_basis_points(*ratio);
                let chunks = split_inner_with_divider(inner, *direction, first_ratio);

                let a_in_first = first.contains_pane_id(pane_a);
                let b_in_first = first.contains_pane_id(pane_b);
                let a_in_second = second.contains_pane_id(pane_a);
                let b_in_second = second.contains_pane_id(pane_b);

                if (a_in_first && b_in_first)
                    && first.resize_between_debug(
                        pane_a,
                        pane_b,
                        side,
                        amount,
                        debug_first_area(area, chunks.divider, *direction),
                    )
                {
                    return true;
                }
                if (a_in_second && b_in_second)
                    && second.resize_between_debug(
                        pane_a,
                        pane_b,
                        side,
                        amount,
                        debug_second_area(area, chunks.divider, *direction),
                    )
                {
                    return true;
                }

                let opposite_sides = (a_in_first && b_in_second) || (a_in_second && b_in_first);
                if !opposite_sides {
                    return false;
                }

                let (current, max_first, total, allowed) = match (*direction, side) {
                    (Direction::Horizontal, SplitSide::Left | SplitSide::Right)
                        if inner.width > 1 =>
                    {
                        let divider_width = u16::from(inner.width >= 3);
                        let available = inner.width.saturating_sub(divider_width);
                        (
                            i32::from(chunks.first.width),
                            i32::from(available.saturating_sub(1)),
                            i32::from(available),
                            available > 1,
                        )
                    }
                    (Direction::Vertical, SplitSide::Top | SplitSide::Bottom)
                        if inner.height > 1 =>
                    {
                        let divider_height = u16::from(inner.height >= 3);
                        let available = inner.height.saturating_sub(divider_height);
                        (
                            i32::from(chunks.first.height),
                            i32::from(available.saturating_sub(1)),
                            i32::from(available),
                            available > 1,
                        )
                    }
                    _ => (0, 0, 0, false),
                };

                if !allowed {
                    return false;
                }

                let signed = match side {
                    SplitSide::Left | SplitSide::Top => -(i32::from(amount)),
                    SplitSide::Right | SplitSide::Bottom => i32::from(amount),
                };
                let next = (current + signed).clamp(1, max_first);
                *ratio = ratio_for_first_rendered_size(next, total);
                true
            }
        }
    }

    pub(crate) fn delete_leaf(&mut self, target_id: usize) -> Option<usize> {
        match self {
            Self::Leaf { pane_id } if *pane_id == target_id => None,
            Self::Leaf { .. } => None,
            Self::Split { first, second, .. } => {
                match &**first {
                    Self::Leaf { pane_id } if *pane_id == target_id => {
                        let sibling =
                            std::mem::replace(second, Box::new(Self::Leaf { pane_id: target_id }));
                        let next_focus = sibling.first_leaf_id();
                        *self = *sibling;
                        return Some(next_focus);
                    }
                    _ => {}
                }

                match &**second {
                    Self::Leaf { pane_id } if *pane_id == target_id => {
                        let sibling =
                            std::mem::replace(first, Box::new(Self::Leaf { pane_id: target_id }));
                        let next_focus = sibling.first_leaf_id();
                        *self = *sibling;
                        return Some(next_focus);
                    }
                    _ => {}
                }

                first
                    .delete_leaf(target_id)
                    .or_else(|| second.delete_leaf(target_id))
            }
        }
    }

    pub(crate) fn first_leaf_id(&self) -> usize {
        match self {
            Self::Leaf { pane_id } => *pane_id,
            Self::Split { first, .. } => first.first_leaf_id(),
        }
    }

    pub(crate) fn max_leaf_id(&self) -> usize {
        match self {
            Self::Leaf { pane_id } => *pane_id,
            Self::Split { first, second, .. } => first.max_leaf_id().max(second.max_leaf_id()),
        }
    }

    pub(crate) fn collect_leaf_ids(&self, out: &mut Vec<usize>) {
        match self {
            Self::Leaf { pane_id } => out.push(*pane_id),
            Self::Split { first, second, .. } => {
                first.collect_leaf_ids(out);
                second.collect_leaf_ids(out);
            }
        }
    }

    pub(crate) fn contains_pane_id(&self, pane_id: usize) -> bool {
        match self {
            Self::Leaf { pane_id: id } => *id == pane_id,
            Self::Split { first, second, .. } => {
                first.contains_pane_id(pane_id) || second.contains_pane_id(pane_id)
            }
        }
    }

    /// True if any vertical (row-wise) split exists. Purely horizontal layouts only partition
    /// width; without a vertical split there is no ratio to adjust for Ctrl+Shift+↑/↓.
    pub(crate) fn has_vertical_split(&self) -> bool {
        match self {
            Self::Leaf { .. } => false,
            Self::Split {
                direction: Direction::Vertical,
                ..
            } => true,
            Self::Split { first, second, .. } => {
                first.has_vertical_split() || second.has_vertical_split()
            }
        }
    }

    pub(crate) fn serialize(&self) -> String {
        match self {
            Self::Leaf { pane_id } => format!("L({})", pane_id),
            Self::Split {
                direction,
                ratio,
                first,
                second,
            } => format!(
                "S({},{},{},{})",
                match direction {
                    Direction::Vertical => 'V',
                    Direction::Horizontal => 'H',
                },
                ratio,
                first.serialize(),
                second.serialize()
            ),
        }
    }

    pub(crate) fn deserialize(input: &str) -> Option<Self> {
        let mut parser = NodeParser::new(input);
        let node = parser.parse_node()?;
        parser.skip_ws();
        if parser.is_eof() {
            Some(node)
        } else {
            None
        }
    }
}

struct NodeParser<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> NodeParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            pos: 0,
        }
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.input.len()
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.input.get(self.pos) {
            if !b.is_ascii_whitespace() {
                break;
            }
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let ch = self.peek()?;
        self.pos += 1;
        Some(ch)
    }

    fn expect(&mut self, expected: u8) -> Option<()> {
        (self.bump()? == expected).then_some(())
    }

    fn parse_usize(&mut self) -> Option<usize> {
        self.skip_ws();
        let start = self.pos;
        while let Some(b) = self.peek() {
            if !b.is_ascii_digit() {
                break;
            }
            self.pos += 1;
        }
        (self.pos > start)
            .then(|| {
                std::str::from_utf8(&self.input[start..self.pos])
                    .ok()?
                    .parse()
                    .ok()
            })
            .flatten()
    }

    fn parse_node(&mut self) -> Option<Node> {
        self.skip_ws();
        match self.bump()? {
            b'L' => {
                self.expect(b'(')?;
                let pane_id = self.parse_usize()?;
                self.skip_ws();
                self.expect(b')')?;
                Some(Node::Leaf { pane_id })
            }
            b'S' => {
                self.expect(b'(')?;
                self.skip_ws();
                let direction = match self.bump()? {
                    b'V' => Direction::Vertical,
                    b'H' => Direction::Horizontal,
                    _ => return None,
                };
                self.skip_ws();
                self.expect(b',')?;
                let ratio = normalize_ratio(self.parse_usize()? as u16);
                self.skip_ws();
                self.expect(b',')?;
                let first = self.parse_node()?;
                self.skip_ws();
                self.expect(b',')?;
                let second = self.parse_node()?;
                self.skip_ws();
                self.expect(b')')?;
                Some(Node::Split {
                    direction,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                })
            }
            _ => None,
        }
    }
}

/// Per-project state lives under `./.codeui/state` so multiple harness
/// instances can run against different projects without sharing layout, and
/// session-resume hints are scoped to the project they came from.
fn layout_persistence_path() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    Some(cwd.join(".codeui").join("state"))
}

pub(crate) fn load_persisted_layout() -> Option<PersistedLayout> {
    let path = layout_persistence_path()?;
    let content = fs::read_to_string(path).ok()?;
    let mut focused = None;
    let mut layout = None;
    let mut default_agent_index = 1usize;
    let mut titles = BTreeMap::new();
    let mut commands = BTreeMap::new();
    let mut resume_commands = BTreeMap::new();

    for line in content.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("focused=") {
            focused = value.trim().parse().ok();
        } else if let Some(value) = line.strip_prefix("layout=") {
            layout = Node::deserialize(value.trim());
        } else if let Some(value) = line.strip_prefix("default_agent=") {
            if let Ok(idx) = value.trim().parse::<usize>() {
                default_agent_index = idx.min(AGENT_PRESETS.len().saturating_sub(1));
            }
        } else if let Some(value) = line.strip_prefix("title=") {
            let Some((id, encoded_title)) = value.split_once(':') else {
                continue;
            };
            let Ok(pane_id) = id.trim().parse() else {
                continue;
            };
            if let Some(title) = decode_persisted_text(encoded_title.trim()) {
                titles.insert(pane_id, title);
            }
        } else if let Some(value) = line.strip_prefix("command=") {
            let Some((id, encoded_command)) = value.split_once(':') else {
                continue;
            };
            let Ok(pane_id) = id.trim().parse() else {
                continue;
            };
            if let Some(command) = decode_persisted_text(encoded_command.trim()) {
                commands.insert(pane_id, command);
            }
        } else if let Some(value) = line.strip_prefix("resume=") {
            let Some((id, encoded_resume)) = value.split_once(':') else {
                continue;
            };
            let Ok(pane_id) = id.trim().parse() else {
                continue;
            };
            if let Some(resume) = decode_persisted_text(encoded_resume.trim()) {
                if !resume.is_empty() {
                    resume_commands.insert(pane_id, resume);
                }
            }
        }
    }

    let layout = layout?;
    let focused = focused
        .filter(|id| layout.contains_pane_id(*id))
        .unwrap_or_else(|| layout.first_leaf_id());

    Some(PersistedLayout {
        layout,
        focused,
        default_agent_index,
        titles,
        commands,
        resume_commands,
    })
}

pub(crate) fn save_persisted_layout(
    layout: &Node,
    focused: usize,
    default_agent_index: usize,
    panes: &[Pane],
) -> io::Result<()> {
    let Some(path) = layout_persistence_path() else {
        return Ok(());
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let default_agent_index = default_agent_index.min(AGENT_PRESETS.len().saturating_sub(1));
    let mut content = format!(
        "focused={}\nlayout={}\ndefault_agent={}\n",
        focused,
        layout.serialize(),
        default_agent_index
    );
    for pane in panes {
        content.push_str(&format!(
            "title={}:{}\n",
            pane.id,
            encode_persisted_text(&pane.title)
        ));
        content.push_str(&format!(
            "command={}:{}\n",
            pane.id,
            encode_persisted_text(&pane.command)
        ));
        if let Some(resume) = &pane.resume_command {
            if !resume.is_empty() {
                content.push_str(&format!(
                    "resume={}:{}\n",
                    pane.id,
                    encode_persisted_text(resume)
                ));
            }
        }
    }

    fs::write(path, content)
}

fn encode_persisted_text(input: &str) -> String {
    let mut out = String::new();
    for byte in input.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            out.push(*byte as char);
        } else {
            out.push('%');
            out.push_str(&format!("{:02X}", byte));
        }
    }
    out
}

fn decode_persisted_text(input: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(input.len());
    let mut chars = input.as_bytes().iter().copied();

    while let Some(byte) = chars.next() {
        if byte == b'%' {
            let hi = chars.next()?;
            let lo = chars.next()?;
            let hex = [hi, lo];
            let value = u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?;
            bytes.push(value);
        } else {
            bytes.push(byte);
        }
    }

    String::from_utf8(bytes).ok()
}

fn contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x
        && x < rect.x.saturating_add(rect.width)
        && y >= rect.y
        && y < rect.y.saturating_add(rect.height)
}

fn inset_rect(area: Rect, amount: u16) -> Rect {
    let inset = amount.min(area.width / 2).min(area.height / 2);
    Rect {
        x: area.x.saturating_add(inset),
        y: area.y.saturating_add(inset),
        width: area.width.saturating_sub(inset.saturating_mul(2)),
        height: area.height.saturating_sub(inset.saturating_mul(2)),
    }
}

fn split_area_by_ratio(area: Rect, direction: Direction, ratio: u16) -> SplitChunks {
    let ratio = ratio_basis_points(ratio);
    match direction {
        Direction::Horizontal => {
            let first_width = proportional_first_size(area.width, ratio);
            SplitChunks {
                first: Rect {
                    x: area.x,
                    y: area.y,
                    width: first_width,
                    height: area.height,
                },
                divider: Rect {
                    x: area.x.saturating_add(first_width),
                    y: area.y,
                    width: u16::from(area.width > 1),
                    height: area.height,
                },
                second: Rect {
                    x: area.x.saturating_add(first_width),
                    y: area.y,
                    width: area.width.saturating_sub(first_width),
                    height: area.height,
                },
            }
        }
        Direction::Vertical => {
            let first_height = proportional_first_size(area.height, ratio);
            SplitChunks {
                first: Rect {
                    x: area.x,
                    y: area.y,
                    width: area.width,
                    height: first_height,
                },
                divider: Rect {
                    x: area.x,
                    y: area.y.saturating_add(first_height),
                    width: area.width,
                    height: u16::from(area.height > 1),
                },
                second: Rect {
                    x: area.x,
                    y: area.y.saturating_add(first_height),
                    width: area.width,
                    height: area.height.saturating_sub(first_height),
                },
            }
        }
    }
}

fn split_inner_with_divider(area: Rect, direction: Direction, ratio: u16) -> SplitChunks {
    let ratio = ratio_basis_points(ratio);
    match direction {
        Direction::Horizontal => {
            let divider_width = u16::from(area.width >= 3);
            let available = area.width.saturating_sub(divider_width);
            let first_width = proportional_first_size(available, ratio);
            let second_width = available.saturating_sub(first_width);
            let divider_x = area.x.saturating_add(first_width);
            SplitChunks {
                first: Rect {
                    x: area.x,
                    y: area.y,
                    width: first_width,
                    height: area.height,
                },
                divider: Rect {
                    x: divider_x,
                    y: area.y,
                    width: divider_width,
                    height: area.height,
                },
                second: Rect {
                    x: divider_x.saturating_add(divider_width),
                    y: area.y,
                    width: second_width,
                    height: area.height,
                },
            }
        }
        Direction::Vertical => {
            let divider_height = u16::from(area.height >= 3);
            let available = area.height.saturating_sub(divider_height);
            let first_height = proportional_first_size(available, ratio);
            let second_height = available.saturating_sub(first_height);
            let divider_y = area.y.saturating_add(first_height);
            SplitChunks {
                first: Rect {
                    x: area.x,
                    y: area.y,
                    width: area.width,
                    height: first_height,
                },
                divider: Rect {
                    x: area.x,
                    y: divider_y,
                    width: area.width,
                    height: divider_height,
                },
                second: Rect {
                    x: area.x,
                    y: divider_y.saturating_add(divider_height),
                    width: area.width,
                    height: second_height,
                },
            }
        }
    }
}

fn split_debug_container_with_divider(
    area: Rect,
    direction: Direction,
    ratio: u16,
) -> Option<SplitChunks> {
    let inner = inset_rect(area, 1);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }

    let chunks = split_inner_with_divider(inner, direction, ratio);
    let divider = debug_divider_area(area, chunks.divider, direction);
    Some(SplitChunks {
        first: debug_first_area(area, divider, direction),
        divider,
        second: debug_second_area(area, divider, direction),
    })
}

fn debug_divider_area(area: Rect, divider: Rect, direction: Direction) -> Rect {
    match direction {
        Direction::Horizontal => Rect {
            x: divider.x,
            y: area.y,
            width: divider.width,
            height: area.height,
        },
        Direction::Vertical => Rect {
            x: area.x,
            y: divider.y,
            width: area.width,
            height: divider.height,
        },
    }
}

fn debug_first_area(area: Rect, divider: Rect, direction: Direction) -> Rect {
    match direction {
        Direction::Horizontal => Rect {
            x: area.x,
            y: area.y,
            width: divider.x.saturating_sub(area.x),
            height: area.height,
        },
        Direction::Vertical => Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: divider.y.saturating_sub(area.y),
        },
    }
}

fn debug_second_area(area: Rect, divider: Rect, direction: Direction) -> Rect {
    match direction {
        Direction::Horizontal => {
            let x = divider.right();
            Rect {
                x,
                y: area.y,
                width: area.right().saturating_sub(x),
                height: area.height,
            }
        }
        Direction::Vertical => {
            let y = divider.bottom();
            Rect {
                x: area.x,
                y,
                width: area.width,
                height: area.bottom().saturating_sub(y),
            }
        }
    }
}

fn proportional_first_size(total: u16, ratio: u16) -> u16 {
    if total <= 1 {
        return total;
    }

    ((u32::from(total) * u32::from(ratio)) / u32::from(RATIO_SCALE))
        .clamp(1, u32::from(total.saturating_sub(1))) as u16
}

fn normalize_ratio(ratio: u16) -> u16 {
    if ratio <= 100 {
        ratio.saturating_mul(RATIO_SCALE / 100)
    } else {
        ratio.min(RATIO_SCALE.saturating_sub(1))
    }
    .max(1)
}

fn ratio_basis_points(ratio: u16) -> u16 {
    normalize_ratio(ratio)
}

fn ratio_for_first_rendered_size(size: i32, total: i32) -> u16 {
    if total <= 0 {
        return 1;
    }

    ((size
        .saturating_mul(i32::from(RATIO_SCALE))
        .saturating_add(total - 1))
        / total)
        .clamp(1, i32::from(RATIO_SCALE.saturating_sub(1))) as u16
}

pub(crate) fn placement_is_adjacent(current: Rect, candidate: Rect, side: SplitSide) -> bool {
    adjacent_overlap(current, candidate, side) > 0
}

pub(crate) fn adjacent_overlap(current: Rect, candidate: Rect, side: SplitSide) -> u16 {
    match side {
        SplitSide::Top => {
            if candidate.bottom() != current.y {
                return 0;
            }
            overlap(candidate.x, candidate.right(), current.x, current.right())
        }
        SplitSide::Bottom => {
            if current.bottom() != candidate.y {
                return 0;
            }
            overlap(candidate.x, candidate.right(), current.x, current.right())
        }
        SplitSide::Left => {
            if candidate.right() != current.x {
                return 0;
            }
            overlap(candidate.y, candidate.bottom(), current.y, current.bottom())
        }
        SplitSide::Right => {
            if current.right() != candidate.x {
                return 0;
            }
            overlap(candidate.y, candidate.bottom(), current.y, current.bottom())
        }
    }
}

pub(crate) fn overlap(start_a: u16, end_a: u16, start_b: u16, end_b: u16) -> u16 {
    let start = start_a.max(start_b);
    let end = end_a.min(end_b);
    end.saturating_sub(start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_left_edge_on_outer_boundary_shrinks_from_the_right() {
        let mut layout = Node::Split {
            direction: Direction::Horizontal,
            ratio: 50,
            first: Box::new(Node::Leaf { pane_id: 0 }),
            second: Box::new(Node::Leaf { pane_id: 1 }),
        };

        assert!(layout.resize_leaf_edge(
            0,
            SplitSide::Left,
            10,
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 20,
            },
            ExposedSides {
                top: true,
                bottom: true,
                left: true,
                right: false,
            },
        ));

        match layout {
            Node::Split { ratio, .. } => assert_eq!(ratio, 4000),
            _ => panic!("expected split"),
        }
    }

    #[test]
    fn resize_right_edge_on_outer_boundary_shrinks_from_the_left() {
        let mut layout = Node::Split {
            direction: Direction::Horizontal,
            ratio: 50,
            first: Box::new(Node::Leaf { pane_id: 0 }),
            second: Box::new(Node::Leaf { pane_id: 1 }),
        };

        assert!(layout.resize_leaf_edge(
            1,
            SplitSide::Right,
            10,
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 20,
            },
            ExposedSides {
                top: true,
                bottom: true,
                left: false,
                right: true,
            },
        ));

        match layout {
            Node::Split { ratio, .. } => assert_eq!(ratio, 6000),
            _ => panic!("expected split"),
        }
    }

    #[test]
    fn resize_vertical_split_adjusts_height_ratio() {
        let mut layout = Node::Split {
            direction: Direction::Vertical,
            ratio: 50,
            first: Box::new(Node::Leaf { pane_id: 0 }),
            second: Box::new(Node::Leaf { pane_id: 1 }),
        };

        assert!(layout.resize_leaf_edge(
            1,
            SplitSide::Top,
            5,
            Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 40,
            },
            ExposedSides {
                top: true,
                bottom: true,
                left: true,
                right: true,
            },
        ));

        match layout {
            Node::Split { ratio, .. } => assert_eq!(ratio, 3750),
            _ => panic!("expected split"),
        }
    }

    #[test]
    fn pure_horizontal_layout_cannot_resize_vertical_edge() {
        let mut layout = Node::Split {
            direction: Direction::Horizontal,
            ratio: 50,
            first: Box::new(Node::Leaf { pane_id: 0 }),
            second: Box::new(Node::Leaf { pane_id: 1 }),
        };

        assert!(!layout.resize_leaf_edge(
            0,
            SplitSide::Bottom,
            5,
            Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 40,
            },
            ExposedSides {
                top: true,
                bottom: true,
                left: true,
                right: false,
            },
        ));

        assert!(!layout.has_vertical_split());
    }

    #[test]
    fn debug_resize_right_moves_divider_one_cell() {
        let mut layout = Node::Split {
            direction: Direction::Horizontal,
            ratio: 50,
            first: Box::new(Node::Leaf { pane_id: 0 }),
            second: Box::new(Node::Leaf { pane_id: 1 }),
        };
        let area = Rect {
            x: 0,
            y: 0,
            width: 12,
            height: 8,
        };

        assert!(layout.resize_between_debug(0, 1, SplitSide::Right, 1, area));

        let mut boundaries = Vec::new();
        layout.collect_debug_resize_boundaries(area, 0, &mut boundaries);
        assert_eq!(boundaries[0].first_area.width, 6);
    }

    #[test]
    fn debug_resize_right_moves_wide_root_divider_one_cell() {
        let mut layout = Node::Split {
            direction: Direction::Horizontal,
            ratio: 50,
            first: Box::new(Node::Leaf { pane_id: 0 }),
            second: Box::new(Node::Leaf { pane_id: 1 }),
        };
        let area = Rect {
            x: 0,
            y: 0,
            width: 202,
            height: 20,
        };

        let mut before = Vec::new();
        layout.collect_debug_resize_boundaries(area, 0, &mut before);
        let divider_x = before[0].divider_area.unwrap().x;

        assert!(layout.resize_between_debug(0, 1, SplitSide::Right, 1, area));

        let mut after = Vec::new();
        layout.collect_debug_resize_boundaries(area, 0, &mut after);
        assert_eq!(after[0].divider_area.unwrap().x, divider_x + 1);
    }

    #[test]
    fn resize_right_moves_wide_root_divider_one_cell() {
        let mut layout = Node::Split {
            direction: Direction::Horizontal,
            ratio: 50,
            first: Box::new(Node::Leaf { pane_id: 0 }),
            second: Box::new(Node::Leaf { pane_id: 1 }),
        };
        let area = Rect {
            x: 0,
            y: 0,
            width: 202,
            height: 20,
        };

        let mut before = Vec::new();
        layout.collect_resize_boundaries(area, 0, &mut before);
        let divider_x = before[0].divider_area.unwrap().x;

        assert!(layout.resize_between(0, 1, SplitSide::Right, 1, area));

        let mut after = Vec::new();
        layout.collect_resize_boundaries(area, 0, &mut after);
        assert_eq!(after[0].divider_area.unwrap().x, divider_x + 1);
    }

    #[test]
    fn debug_resize_down_moves_divider_one_cell() {
        let mut layout = Node::Split {
            direction: Direction::Vertical,
            ratio: 50,
            first: Box::new(Node::Leaf { pane_id: 0 }),
            second: Box::new(Node::Leaf { pane_id: 1 }),
        };
        let area = Rect {
            x: 0,
            y: 0,
            width: 12,
            height: 8,
        };

        assert!(layout.resize_between_debug(0, 1, SplitSide::Bottom, 1, area));

        let mut boundaries = Vec::new();
        layout.collect_debug_resize_boundaries(area, 0, &mut boundaries);
        assert_eq!(boundaries[0].first_area.height, 4);
    }

    #[test]
    fn debug_divider_endpoints_overlap_container_edges() {
        let horizontal = Node::Split {
            direction: Direction::Horizontal,
            ratio: 50,
            first: Box::new(Node::Leaf { pane_id: 0 }),
            second: Box::new(Node::Leaf { pane_id: 1 }),
        };
        let vertical = Node::Split {
            direction: Direction::Vertical,
            ratio: 50,
            first: Box::new(Node::Leaf { pane_id: 0 }),
            second: Box::new(Node::Leaf { pane_id: 1 }),
        };
        let area = Rect {
            x: 3,
            y: 2,
            width: 20,
            height: 10,
        };

        let mut horizontal_boundaries = Vec::new();
        horizontal.collect_debug_resize_boundaries(area, 0, &mut horizontal_boundaries);
        assert_eq!(horizontal_boundaries[0].divider_area.unwrap().y, area.y);
        assert_eq!(
            horizontal_boundaries[0].divider_area.unwrap().height,
            area.height
        );

        let mut vertical_boundaries = Vec::new();
        vertical.collect_debug_resize_boundaries(area, 0, &mut vertical_boundaries);
        assert_eq!(vertical_boundaries[0].divider_area.unwrap().x, area.x);
        assert_eq!(
            vertical_boundaries[0].divider_area.unwrap().width,
            area.width
        );
    }
}
