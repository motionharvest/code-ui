use std::{collections::BTreeMap, fs, io, path::PathBuf};

use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::pane::Pane;

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
    pub(crate) titles: BTreeMap<usize, String>,
}

pub(crate) struct Placement {
    pub(crate) pane_id: usize,
    pub(crate) area: Rect,
    pub(crate) exposed: ExposedSides,
}

impl Placement {
    pub(crate) fn plus_hit(&self, x: u16, y: u16) -> Option<SplitSide> {
        if self.area.width >= 3 {
            let center_x = self.area.x + (self.area.width / 2);
            if self.exposed.top && y == self.area.y && x == center_x {
                return Some(SplitSide::Top);
            }
            if self.exposed.bottom && y == self.area.bottom().saturating_sub(1) && x == center_x {
                return Some(SplitSide::Bottom);
            }
        }

        if self.area.height >= 3 {
            let center_y = self.area.y + (self.area.height / 2);
            if self.exposed.left && x == self.area.x && y == center_y {
                return Some(SplitSide::Left);
            }
            if self.exposed.right && x == self.area.right().saturating_sub(1) && y == center_y {
                return Some(SplitSide::Right);
            }
        }

        None
    }

    pub(crate) fn title_hit(&self, title: &str, focused: bool, x: u16, y: u16) -> bool {
        if y != self.area.y || self.area.width <= 2 {
            return false;
        }

        let display_width = title.chars().count() + usize::from(focused);
        let title_width = (display_width as u16).min(self.area.width.saturating_sub(2));
        contains(
            Rect {
                x: self.area.x + 1,
                y: self.area.y,
                width: title_width,
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
                let first_ratio = (*ratio).clamp(1, 99);
                let chunks = Layout::default()
                    .direction(*direction)
                    .constraints([
                        Constraint::Percentage(first_ratio),
                        Constraint::Percentage(100 - first_ratio),
                    ])
                    .split(area);

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

                first.collect(chunks[0], first_exposed, out);
                second.collect(chunks[1], second_exposed, out);
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
                        ratio: 50,
                        first: Box::new(Self::Leaf { pane_id: new_id }),
                        second: Box::new(Self::Leaf { pane_id: old_id }),
                    },
                    SplitSide::Bottom => Self::Split {
                        direction: Direction::Vertical,
                        ratio: 50,
                        first: Box::new(Self::Leaf { pane_id: old_id }),
                        second: Box::new(Self::Leaf { pane_id: new_id }),
                    },
                    SplitSide::Left => Self::Split {
                        direction: Direction::Horizontal,
                        ratio: 50,
                        first: Box::new(Self::Leaf { pane_id: new_id }),
                        second: Box::new(Self::Leaf { pane_id: old_id }),
                    },
                    SplitSide::Right => Self::Split {
                        direction: Direction::Horizontal,
                        ratio: 50,
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
            .then(|| std::str::from_utf8(&self.input[start..self.pos]).ok()?.parse().ok())
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
                let ratio = self.parse_usize()? as u16;
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

fn layout_persistence_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("split_tui")
            .join("layout"),
    )
}

pub(crate) fn load_persisted_layout() -> Option<PersistedLayout> {
    let path = layout_persistence_path()?;
    let content = fs::read_to_string(path).ok()?;
    let mut focused = None;
    let mut layout = None;
    let mut titles = BTreeMap::new();

    for line in content.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("focused=") {
            focused = value.trim().parse().ok();
        } else if let Some(value) = line.strip_prefix("layout=") {
            layout = Node::deserialize(value.trim());
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
        }
    }

    let layout = layout?;
    let focused = focused
        .filter(|id| layout.contains_pane_id(*id))
        .unwrap_or_else(|| layout.first_leaf_id());

    Some(PersistedLayout {
        layout,
        focused,
        titles,
    })
}

pub(crate) fn save_persisted_layout(layout: &Node, focused: usize, panes: &[Pane]) -> io::Result<()> {
    let Some(path) = layout_persistence_path() else {
        return Ok(());
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut content = format!("focused={}\nlayout={}\n", focused, layout.serialize());
    for pane in panes {
        content.push_str(&format!(
            "title={}:{}\n",
            pane.id,
            encode_persisted_text(&pane.title)
        ));
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
