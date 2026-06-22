//! The split-tree pane layout (§2.1): a tab holds a binary tree of splits whose
//! leaves are panes. Pure geometry and tree surgery, independent of any renderer.

// ========================================================================
// Data Structures
// ========================================================================

/// One tab's pane layout: a binary split tree plus which leaf has focus.
/// `PaneId`s are allocated by the owner (so they stay unique across tabs) and
/// passed into [`Tab::with_root`] and [`Tab::split`].
#[derive(Clone, Debug)]
pub struct Tab {
    focused: PaneId,
    root: Node,
    /// When true, `rects()` returns only the focused pane at the full viewport;
    /// cleared when the user calls `toggle_zoom()` again.
    zoomed: bool,
}

/// A node in the split tree: a pane leaf or a binary split.
#[derive(Clone, Debug)]
enum Node {
    Leaf(PaneId),
    Split(SplitNode),
}

/// An internal split dividing its area between two child nodes.
#[derive(Clone, Debug)]
struct SplitNode {
    direction: Direction,
    first: Box<Node>,
    ratio: f32,
    second: Box<Node>,
}

/// A rectangular area, in the renderer's coordinate space (origin top-left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub height: f32,
    pub width: f32,
    pub x: f32,
    pub y: f32,
}

/// Which way a split's divider runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    /// A horizontal divider: first child on top, second below.
    Horizontal,
    /// A vertical divider: first child on the left, second on the right.
    Vertical,
}

/// A directional focus move.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FocusDir {
    Down,
    Left,
    Right,
    Up,
}

/// Identifies a pane within a tab.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PaneId(pub u64);

// ========================================================================
// Tab
// ========================================================================

impl Tab {
    /// A tab whose single full-area pane is `PaneId(0)`, focused.
    pub fn new() -> Self {
        Self::with_root(PaneId(0))
    }

    /// A tab with a single full-area pane `root`, focused.
    pub fn with_root(root: PaneId) -> Self {
        Self {
            focused: root,
            root: Node::Leaf(root),
            zoomed: false,
        }
    }

    /// The currently focused pane.
    pub fn focused(&self) -> PaneId {
        self.focused
    }

    /// Every pane, left-to-right / top-to-bottom in tree order.
    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        collect_panes(&self.root, &mut out);
        out
    }

    /// Each pane paired with its area within `viewport`. When zoomed, only the
    /// focused pane is returned and it occupies the entire viewport.
    pub fn rects(&self, viewport: Rect) -> Vec<(PaneId, Rect)> {
        if self.zoomed {
            return vec![(self.focused, viewport)];
        }
        let mut out = Vec::new();
        collect_rects(&self.root, viewport, &mut out);
        out
    }

    /// Toggle the focused pane between full-viewport zoom and normal split layout.
    pub fn toggle_zoom(&mut self) {
        self.zoomed = !self.zoomed;
    }

    /// Whether the focused pane is currently expanded to fill the full viewport.
    pub fn is_zoomed(&self) -> bool {
        self.zoomed
    }

    /// Split the focused pane in two, placing the caller-allocated `new_id` as
    /// the new leaf and focusing it.
    pub fn split(&mut self, direction: Direction, ratio: f32, new_id: PaneId) {
        split_at(
            &mut self.root,
            self.focused,
            direction,
            ratio.clamp(0.0, 1.0),
            new_id,
        );
        self.focused = new_id;
    }

    /// Close a pane, collapsing its parent split into its sibling. The last pane
    /// cannot be closed. Returns whether anything changed.
    pub fn close(&mut self, pane: PaneId) -> bool {
        if !close_in(&mut self.root, pane) {
            return false;
        }
        if self.focused == pane {
            self.focused = self.panes().first().copied().unwrap_or(PaneId(0));
        }
        true
    }

    /// Focus a specific pane if it exists.
    pub fn focus(&mut self, pane: PaneId) -> bool {
        if self.panes().contains(&pane) {
            self.focused = pane;
            return true;
        }
        false
    }

    /// Focus the next pane in tree order, wrapping around.
    pub fn focus_next(&mut self) {
        let panes = self.panes();
        if let Some(index) = panes.iter().position(|&p| p == self.focused) {
            self.focused = panes[(index + 1) % panes.len()];
        }
    }

    /// Focus the nearest pane in the given direction within `viewport`, by the
    /// distance between pane centers. Returns whether focus moved.
    pub fn focus_in_direction(&mut self, direction: FocusDir, viewport: Rect) -> bool {
        let rects = self.rects(viewport);
        let Some(current) = rects.iter().find(|(id, _)| *id == self.focused) else {
            return false;
        };
        let from = current.1.center();

        let best = rects
            .iter()
            .filter(|(id, _)| *id != self.focused)
            .filter(|(_, rect)| is_toward(direction, from, rect.center()))
            .min_by(|a, b| distance(from, a.1.center()).total_cmp(&distance(from, b.1.center())));

        match best {
            Some((id, _)) => {
                self.focused = *id;
                true
            }
            None => false,
        }
    }
}

impl Default for Tab {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// Rect
// ========================================================================

impl Rect {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            height,
            width,
            x,
            y,
        }
    }

    fn center(self) -> (f32, f32) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    fn split(self, direction: Direction, ratio: f32) -> (Rect, Rect) {
        match direction {
            Direction::Vertical => {
                let width = self.width * ratio;
                (
                    Rect::new(self.x, self.y, width, self.height),
                    Rect::new(self.x + width, self.y, self.width - width, self.height),
                )
            }
            Direction::Horizontal => {
                let height = self.height * ratio;
                (
                    Rect::new(self.x, self.y, self.width, height),
                    Rect::new(self.x, self.y + height, self.width, self.height - height),
                )
            }
        }
    }
}

// ========================================================================
// Tree helpers
// ========================================================================

fn collect_panes(node: &Node, out: &mut Vec<PaneId>) {
    match node {
        Node::Leaf(id) => out.push(*id),
        Node::Split(split) => {
            collect_panes(&split.first, out);
            collect_panes(&split.second, out);
        }
    }
}

fn collect_rects(node: &Node, area: Rect, out: &mut Vec<(PaneId, Rect)>) {
    match node {
        Node::Leaf(id) => out.push((*id, area)),
        Node::Split(split) => {
            let (first, second) = area.split(split.direction, split.ratio);
            collect_rects(&split.first, first, out);
            collect_rects(&split.second, second, out);
        }
    }
}

fn split_at(
    node: &mut Node,
    target: PaneId,
    direction: Direction,
    ratio: f32,
    new_id: PaneId,
) -> bool {
    match node {
        Node::Leaf(id) if *id == target => {
            *node = Node::Split(SplitNode {
                direction,
                first: Box::new(Node::Leaf(target)),
                ratio,
                second: Box::new(Node::Leaf(new_id)),
            });
            true
        }
        Node::Leaf(_) => false,
        Node::Split(split) => {
            split_at(&mut split.first, target, direction, ratio, new_id)
                || split_at(&mut split.second, target, direction, ratio, new_id)
        }
    }
}

fn close_in(node: &mut Node, target: PaneId) -> bool {
    let replacement = match node {
        Node::Leaf(_) => return false,
        Node::Split(split) if leaf_is(&split.first, target) => {
            std::mem::replace(split.second.as_mut(), Node::Leaf(target))
        }
        Node::Split(split) if leaf_is(&split.second, target) => {
            std::mem::replace(split.first.as_mut(), Node::Leaf(target))
        }
        Node::Split(split) => {
            return close_in(&mut split.first, target) || close_in(&mut split.second, target);
        }
    };
    *node = replacement;
    true
}

fn leaf_is(node: &Node, target: PaneId) -> bool {
    matches!(node, Node::Leaf(id) if *id == target)
}

fn is_toward(direction: FocusDir, from: (f32, f32), to: (f32, f32)) -> bool {
    match direction {
        FocusDir::Down => to.1 > from.1,
        FocusDir::Left => to.0 < from.0,
        FocusDir::Right => to.0 > from.0,
        FocusDir::Up => to.1 < from.1,
    }
}

fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    dx * dx + dy * dy
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const VIEWPORT: Rect = Rect {
        height: 100.0,
        width: 200.0,
        x: 0.0,
        y: 0.0,
    };

    #[test]
    fn test_new_tab_has_one_focused_pane() {
        let tab = Tab::new();
        assert_eq!(tab.panes(), vec![PaneId(0)]);
        assert_eq!(tab.focused(), PaneId(0));
    }

    #[test]
    fn test_split_adds_a_focused_pane_and_divides_the_area() {
        let mut tab = Tab::new();
        let right = PaneId(1);
        tab.split(Direction::Vertical, 0.5, right);
        assert_eq!(tab.focused(), right);
        assert_eq!(tab.panes(), vec![PaneId(0), right]);

        let rects = tab.rects(VIEWPORT);
        assert_eq!(rects[0], (PaneId(0), Rect::new(0.0, 0.0, 100.0, 100.0)));
        assert_eq!(rects[1], (right, Rect::new(100.0, 0.0, 100.0, 100.0)));
    }

    #[test]
    fn test_close_collapses_split_into_sibling() {
        let mut tab = Tab::new();
        let right = PaneId(1);
        tab.split(Direction::Vertical, 0.5, right);
        assert!(tab.close(right));
        assert_eq!(tab.panes(), vec![PaneId(0)]);
        assert_eq!(tab.focused(), PaneId(0));
        assert_eq!(tab.rects(VIEWPORT), vec![(PaneId(0), VIEWPORT)]);
    }

    #[test]
    fn test_last_pane_cannot_be_closed() {
        let mut tab = Tab::new();
        assert!(!tab.close(PaneId(0)));
        assert_eq!(tab.panes(), vec![PaneId(0)]);
    }

    #[test]
    fn test_focus_next_wraps_around() {
        let mut tab = Tab::new();
        let right = PaneId(1);
        tab.split(Direction::Vertical, 0.5, right);
        tab.focus(PaneId(0));
        tab.focus_next();
        assert_eq!(tab.focused(), right);
        tab.focus_next();
        assert_eq!(tab.focused(), PaneId(0));
    }

    #[test]
    fn test_zoom_returns_full_viewport_for_focused_pane() {
        let mut tab = Tab::new();
        let right = PaneId(1);
        tab.split(Direction::Vertical, 0.5, right);
        tab.focus(PaneId(0));
        assert!(!tab.is_zoomed());
        tab.toggle_zoom();
        assert!(tab.is_zoomed());
        let rects = tab.rects(VIEWPORT);
        assert_eq!(rects.len(), 1, "only focused pane when zoomed");
        assert_eq!(rects[0], (PaneId(0), VIEWPORT));
        tab.toggle_zoom();
        assert!(!tab.is_zoomed());
        let rects = tab.rects(VIEWPORT);
        assert_eq!(rects.len(), 2, "both panes restored after unzoom");
    }

    #[test]
    fn test_focus_in_direction_moves_to_the_adjacent_pane() {
        let mut tab = Tab::new();
        let right = PaneId(1);
        tab.split(Direction::Vertical, 0.5, right);
        tab.focus(PaneId(0));
        assert!(tab.focus_in_direction(FocusDir::Right, VIEWPORT));
        assert_eq!(tab.focused(), right);
        assert!(!tab.focus_in_direction(FocusDir::Right, VIEWPORT));
        assert!(tab.focus_in_direction(FocusDir::Left, VIEWPORT));
        assert_eq!(tab.focused(), PaneId(0));
    }
}
