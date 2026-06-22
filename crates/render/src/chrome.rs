//! Top window chrome: the tabbar and menubar model plus their pixel geometry.
//!
//! Geometry lives here, in one place, so the GPU renderer (which draws the
//! chrome) and the app (which hit-tests mouse clicks against it) compute the
//! exact same rectangles and never drift. The app supplies a [`TopChrome`]
//! describing the tabs and menus; [`layout`] turns it into concrete pixel
//! [`Region`]s, and [`hit_test`] maps a click to a [`ChromeHit`].

// ========================================================================
// Constants
// ========================================================================

/// Tab width, in cells. Tabs are a fixed width; titles are truncated to fit.
const TAB_CELLS: f32 = 18.0;
/// Width of the trailing close (`×`) target inside a tab, in cells.
const CLOSE_CELLS: f32 = 2.0;
/// Width of the new-tab (`+`) button, in cells.
const NEW_TAB_CELLS: f32 = 3.0;
/// Width of the modern hamburger (`☰`) button, in cells.
const HAMBURGER_CELLS: f32 = 3.0;
/// Width of each window-control button (minimize/maximize/close), in cells.
const CONTROL_CELLS: f32 = 3.0;
/// Horizontal padding around a classic menu title, in cells (one each side).
const MENU_TITLE_PAD_CELLS: f32 = 2.0;
/// Minimum dropdown panel width, in cells.
const DROPDOWN_MIN_CELLS: f32 = 22.0;
/// Padding added to the widest `label` + `shortcut` when sizing a dropdown, in
/// cells (leading indent, gap between label and shortcut, trailing margin).
const DROPDOWN_PAD_CELLS: f32 = 4.0;
/// Height of one dropdown item row as a multiple of the cell height. Taller than
/// a terminal row so the menu reads as an app menu, not a packed text grid.
const DROPDOWN_ITEM_RATIO: f32 = 1.9;
/// Padding above the first and below the last dropdown item, in cells.
const DROPDOWN_PAD_Y_CELLS: f32 = 0.4;

// ========================================================================
// Data Structures
// ========================================================================

/// Which menubar presentation to draw.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MenuStyle {
    /// A `File / Edit / View / …` row above the tabbar; each title opens its menu.
    Classic,
    /// A single `☰` button on the tabbar that opens one dropdown of commands.
    #[default]
    Modern,
}

/// Which edge of the title bar carries the minimize/maximize/close buttons.
/// The hamburger button (modern style only) sits on the opposite edge.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ControlsSide {
    /// Window controls hug the left edge; hamburger (if any) on the right.
    #[default]
    Left,
    /// Window controls hug the right edge; hamburger (if any) on the left.
    Right,
}

/// One tab's label in the tabbar.
#[derive(Clone, Debug)]
pub struct TabLabel {
    /// When true, a small bell dot is drawn on the tab to signal an unread notification.
    pub bell: bool,
    pub title: String,
}

/// One dropdown menu: a `title` (shown only in the classic menubar) and its
/// items. Item order matches the app's parallel command list.
#[derive(Clone, Debug)]
pub struct Menu {
    pub items: Vec<MenuItem>,
    pub title: String,
}

/// One selectable line in a dropdown. Purely presentational; the app maps the
/// same index to a command name. An item with `children` is a submenu parent:
/// hovering it opens a child panel to the right instead of running a command.
#[derive(Clone, Debug)]
pub struct MenuItem {
    pub children: Vec<MenuItem>,
    pub label: String,
    pub shortcut: String,
}

impl MenuItem {
    /// Whether this item opens a submenu rather than dispatching a command.
    pub fn has_children(&self) -> bool {
        !self.children.is_empty()
    }
}

/// The full top-chrome model the app hands the renderer each frame.
#[derive(Clone, Debug)]
pub struct TopChrome {
    pub active_tab: usize,
    /// Tab index whose bell dot is being hovered; drives tooltip rendering.
    pub bell_tooltip_tab: Option<usize>,
    /// Which edge the window controls occupy; the hamburger button (modern style
    /// only) is placed on the opposite edge.
    pub controls_side: ControlsSide,
    pub menu_style: MenuStyle,
    pub menus: Vec<Menu>,
    /// Index into `menus` of the open dropdown, or `None` when closed.
    pub open_menu: Option<usize>,
    /// Index (into the open menu's `items`) of the parent whose submenu is shown,
    /// or `None` when no submenu is open.
    pub open_submenu: Option<usize>,
    /// The highlighted dropdown item (mouse hover), if any.
    pub selected_item: Option<usize>,
    /// The highlighted submenu child (mouse hover), if any.
    pub selected_subitem: Option<usize>,
    pub tabs: Vec<TabLabel>,
    /// Whether to draw custom minimize/maximize/close controls (the borderless
    /// "modern" title bar); `false` lets the OS draw them.
    pub window_controls: bool,
}

/// A pixel rectangle in surface coordinates (origin top-left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Region {
    pub h: f32,
    pub w: f32,
    pub x: f32,
    pub y: f32,
}

/// Geometry of an open dropdown panel and its item rows.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DropdownLayout {
    pub item_h: f32,
    pub items: usize,
    pub origin_x: f32,
    /// Vertical padding inside the panel, above the first and below the last row.
    pub pad: f32,
    pub top: f32,
    pub width: f32,
}

/// Concrete pixel geometry of the whole chrome for one frame.
pub(crate) struct ChromeLayout {
    /// Bell-dot hit targets, parallel to `tabs`; only present for tabs with `bell: true`.
    pub bell_dots: Vec<Option<Region>>,
    /// Per-tab close (`×`) targets, parallel to `tabs`.
    pub closes: Vec<Region>,
    /// The `[minimize, maximize, close]` window-control targets at the right edge,
    /// or `None` when the OS draws the decorations.
    pub controls: Option<[Region; 3]>,
    pub dropdown: Option<DropdownLayout>,
    /// The modern hamburger button, or `None` in classic style.
    pub hamburger: Option<Region>,
    /// Classic menubar band top (`y`), or `None` in modern style.
    pub menubar_top: Option<f32>,
    /// Classic menu-title targets, parallel to `menus`; empty in modern style.
    pub menu_titles: Vec<Region>,
    pub new_tab: Region,
    /// The child panel of the open submenu parent, to the right of `dropdown`.
    pub submenu: Option<DropdownLayout>,
    /// Top (`y`) of the tabbar row.
    pub tab_row_top: f32,
    pub tabs: Vec<Region>,
}

/// What a click landed on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChromeHit {
    /// A bell-notification dot on a tab, by tab index.
    BellDot(usize),
    /// The window close (`✕`) control.
    Close,
    CloseTab(usize),
    /// An item in the open dropdown, by index into that menu's `items`.
    DropdownItem(usize),
    Hamburger,
    /// The window maximize/restore (`□`) control.
    Maximize,
    /// A classic menu title, by index into `menus`.
    MenuTitle(usize),
    /// The window minimize (`—`) control.
    Minimize,
    NewTab,
    None,
    /// A child of the open submenu, by index into that submenu's items. The
    /// parent is the chrome's `open_submenu`.
    SubmenuItem(usize),
    Tab(usize),
}

// ========================================================================
// Region
// ========================================================================

impl Region {
    fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

// ========================================================================
// Public API
// ========================================================================

/// Number of top cell rows the chrome occupies: the tabbar, plus the menubar row
/// in classic style.
pub fn chrome_rows(style: MenuStyle) -> usize {
    match style {
        // Classic stacks a menubar over the tabbar. Modern is a single bar, two
        // cells tall so the title bar has room to breathe (VS Code-ish height).
        MenuStyle::Classic => 2,
        MenuStyle::Modern => 2,
    }
}

/// Map a click at `(x, y)` to the chrome element under it. The open submenu wins
/// first (it overlays the parent), then the parent dropdown, then per-tab close
/// buttons, tabs, the new-tab button, and finally the menu triggers.
pub fn hit_test(
    chrome: &TopChrome,
    surface_w: f32,
    cell_w: f32,
    cell_h: f32,
    x: f32,
    y: f32,
) -> ChromeHit {
    let layout = layout(chrome, surface_w, cell_w, cell_h);

    if let Some(submenu) = &layout.submenu {
        for i in 0..submenu.items {
            if dropdown_item_region(submenu, i).contains(x, y) {
                return ChromeHit::SubmenuItem(i);
            }
        }
    }
    if let Some(dropdown) = &layout.dropdown {
        for i in 0..dropdown.items {
            if dropdown_item_region(dropdown, i).contains(x, y) {
                return ChromeHit::DropdownItem(i);
            }
        }
    }

    if let Some([minimize, maximize, close]) = &layout.controls {
        if close.contains(x, y) {
            return ChromeHit::Close;
        }
        if maximize.contains(x, y) {
            return ChromeHit::Maximize;
        }
        if minimize.contains(x, y) {
            return ChromeHit::Minimize;
        }
    }

    for (i, region) in layout.closes.iter().enumerate() {
        if region.contains(x, y) {
            return ChromeHit::CloseTab(i);
        }
    }
    for (i, dot) in layout.bell_dots.iter().enumerate() {
        if dot.is_some_and(|r| r.contains(x, y)) {
            return ChromeHit::BellDot(i);
        }
    }
    for (i, region) in layout.tabs.iter().enumerate() {
        if region.contains(x, y) {
            return ChromeHit::Tab(i);
        }
    }
    if layout.new_tab.contains(x, y) {
        return ChromeHit::NewTab;
    }
    if let Some(hamburger) = &layout.hamburger {
        if hamburger.contains(x, y) {
            return ChromeHit::Hamburger;
        }
    }
    for (i, region) in layout.menu_titles.iter().enumerate() {
        if region.contains(x, y) {
            return ChromeHit::MenuTitle(i);
        }
    }
    ChromeHit::None
}

// ========================================================================
// Layout
// ========================================================================

/// Compute the full pixel geometry of the chrome. Shared by [`hit_test`] and the
/// renderer so both agree on every rectangle.
pub(crate) fn layout(chrome: &TopChrome, surface_w: f32, cw: f32, ch: f32) -> ChromeLayout {
    let classic = chrome.menu_style == MenuStyle::Classic;
    let chrome_h = chrome_rows(chrome.menu_style) as f32 * ch;
    let menubar_top = if classic { Some(0.0) } else { None };
    let tab_row_top = if classic { ch } else { 0.0 };
    // Interactive elements are one cell tall in classic (one row each) but span
    // the whole taller bar in modern, so clicks land anywhere in the band.
    let bar_h = if classic { ch } else { chrome_h };

    // The modern hamburger sits on the edge opposite the window controls; the
    // tabs begin after whichever element is on the left. Classic style has no
    // hamburger, but window controls (if enabled) still reserve their edge.
    let controls_left = chrome.controls_side == ControlsSide::Left;
    let hamburger_w = HAMBURGER_CELLS * cw;
    let left_reserve = if controls_left {
        if chrome.window_controls {
            3.0 * CONTROL_CELLS * cw
        } else {
            0.0
        }
    } else {
        hamburger_w
    };

    let hamburger = (!classic).then_some(Region {
        h: bar_h,
        w: hamburger_w,
        x: if controls_left {
            surface_w - hamburger_w
        } else {
            0.0
        },
        y: tab_row_top,
    });
    let tabs_left = left_reserve;

    let tab_w = TAB_CELLS * cw;
    let mut tabs = Vec::with_capacity(chrome.tabs.len());
    let mut closes = Vec::with_capacity(chrome.tabs.len());
    for i in 0..chrome.tabs.len() {
        let x = tabs_left + i as f32 * tab_w;
        tabs.push(Region {
            h: bar_h,
            w: tab_w,
            x,
            y: tab_row_top,
        });
        closes.push(Region {
            h: bar_h,
            w: CLOSE_CELLS * cw,
            x,
            y: tab_row_top,
        });
    }

    let new_tab = Region {
        h: bar_h,
        w: NEW_TAB_CELLS * cw,
        x: tabs_left + chrome.tabs.len() as f32 * tab_w,
        y: tab_row_top,
    };

    // Window controls hug the edge chosen by `controls_side`: minimize,
    // maximize, then close. Only present in the borderless modern title bar.
    let controls = chrome.window_controls.then(|| {
        let w = CONTROL_CELLS * cw;
        let control = |slot: f32| Region {
            h: bar_h,
            w,
            x: if controls_left {
                slot * w
            } else {
                surface_w - (3.0 - slot) * w
            },
            y: tab_row_top,
        };
        [control(0.0), control(1.0), control(2.0)]
    });

    let menu_titles = if classic {
        let mut titles = Vec::with_capacity(chrome.menus.len());
        let mut x = 0.0;
        for menu in &chrome.menus {
            let w = (menu.title.chars().count() as f32 + MENU_TITLE_PAD_CELLS) * cw;
            titles.push(Region {
                h: ch,
                w,
                x,
                y: 0.0,
            });
            x += w;
        }
        titles
    } else {
        Vec::new()
    };

    let dropdown = chrome.open_menu.and_then(|open| {
        let menu = chrome.menus.get(open)?;
        let width = panel_width(&menu.items, cw).min(surface_w);
        let (origin_x, top) = if classic {
            let title = menu_titles.get(open)?;
            (title.x, ch)
        } else {
            // Anchor to the hamburger: when it is on the left (controls right,
            // the default) the dropdown opens flush with its left edge growing
            // right; when it is on the right (controls left) the dropdown right-
            // aligns with the hamburger so it stays on screen.
            let hb = hamburger.as_ref()?;
            let x = if controls_left {
                (hb.x + hb.w - width).max(0.0)
            } else {
                hb.x
            };
            (x, chrome_h)
        };
        Some(DropdownLayout {
            item_h: ch * DROPDOWN_ITEM_RATIO,
            items: menu.items.len(),
            origin_x,
            pad: ch * DROPDOWN_PAD_Y_CELLS,
            top,
            width,
        })
    });

    // The submenu opens beside the parent panel — to the right by default, or
    // to the left when controls are on the left edge (so it does not run off the
    // right side of the window). Its first child row aligns with the hovered
    // parent item; the parent panel stays put.
    let submenu = dropdown.and_then(|parent| {
        let open = chrome.open_menu?;
        let parent_idx = chrome.open_submenu?;
        let item = chrome.menus.get(open)?.items.get(parent_idx)?;
        if item.children.is_empty() {
            return None;
        }
        let child_width = panel_width(&item.children, cw).min(surface_w);
        let origin_x = if controls_left {
            (parent.origin_x - child_width).max(0.0)
        } else {
            parent.origin_x + parent.width
        };
        Some(DropdownLayout {
            item_h: parent.item_h,
            items: item.children.len(),
            origin_x,
            pad: parent.pad,
            top: parent.top + parent_idx as f32 * parent.item_h,
            width: child_width,
        })
    });

    // Bell-dot hit regions: top-right quadrant of each tab that has bell active.
    let bell_dots = tabs
        .iter()
        .enumerate()
        .map(|(i, tab)| {
            chrome.tabs.get(i).filter(|t| t.bell).map(|_| Region {
                h: tab.h * 0.45,
                w: tab.w * 0.35,
                x: tab.x + tab.w * 0.65,
                y: tab.y,
            })
        })
        .collect();

    ChromeLayout {
        bell_dots,
        closes,
        controls,
        dropdown,
        hamburger,
        menubar_top,
        menu_titles,
        new_tab,
        submenu,
        tab_row_top,
        tabs,
    }
}

/// The pixel rect of dropdown item `i`.
pub(crate) fn dropdown_item_region(dropdown: &DropdownLayout, i: usize) -> Region {
    Region {
        h: dropdown.item_h,
        w: dropdown.width,
        x: dropdown.origin_x,
        y: dropdown.top + dropdown.pad + i as f32 * dropdown.item_h,
    }
}

/// Panel width sized to the widest `label` + `shortcut`, floored at a minimum.
fn panel_width(items: &[MenuItem], cw: f32) -> f32 {
    let widest = items
        .iter()
        .map(|item| item.label.chars().count() + item.shortcut.chars().count())
        .max()
        .unwrap_or(0) as f32;
    (widest + DROPDOWN_PAD_CELLS).max(DROPDOWN_MIN_CELLS) * cw
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const CW: f32 = 10.0;
    const CH: f32 = 20.0;
    const SURFACE_W: f32 = 1200.0;

    fn leaf(label: &str, shortcut: &str) -> MenuItem {
        MenuItem {
            children: Vec::new(),
            label: label.into(),
            shortcut: shortcut.into(),
        }
    }

    fn chrome(style: MenuStyle, tabs: usize, open: Option<usize>) -> TopChrome {
        let menus = match style {
            MenuStyle::Modern => vec![Menu {
                title: "Menu".into(),
                items: vec![
                    leaf("New Tab", "Ctrl-Shift-T"),
                    // A submenu parent: hovering it opens its children.
                    MenuItem {
                        children: vec![leaf("Vertical", ""), leaf("Horizontal", "")],
                        label: "Split".into(),
                        shortcut: String::new(),
                    },
                ],
            }],
            MenuStyle::Classic => vec![
                Menu {
                    title: "File".into(),
                    items: vec![leaf("New Tab", "")],
                },
                Menu {
                    title: "View".into(),
                    items: vec![leaf("Theme", "")],
                },
            ],
        };
        TopChrome {
            active_tab: 0,
            bell_tooltip_tab: None,
            controls_side: ControlsSide::Right,
            menu_style: style,
            menus,
            open_menu: open,
            open_submenu: None,
            selected_item: None,
            selected_subitem: None,
            tabs: (0..tabs)
                .map(|i| TabLabel {
                    bell: false,
                    title: format!("Tab {i}"),
                })
                .collect(),
            window_controls: false,
        }
    }

    #[test]
    fn test_chrome_rows_by_style() {
        assert_eq!(chrome_rows(MenuStyle::Modern), 2);
        assert_eq!(chrome_rows(MenuStyle::Classic), 2);
    }

    #[test]
    fn test_modern_tabbar_is_top_row_classic_is_second() {
        let modern = layout(&chrome(MenuStyle::Modern, 2, None), SURFACE_W, CW, CH);
        assert_eq!(modern.tab_row_top, 0.0);
        assert_eq!(modern.menubar_top, None);

        let classic = layout(&chrome(MenuStyle::Classic, 2, None), SURFACE_W, CW, CH);
        assert_eq!(classic.tab_row_top, CH);
        assert_eq!(classic.menubar_top, Some(0.0));
    }

    #[test]
    fn test_hit_test_picks_tab_then_new_tab() {
        let c = chrome(MenuStyle::Modern, 2, None);
        // The modern hamburger occupies the first 3 cells; tabs follow it.
        // Each tab has a CLOSE_CELLS-wide close button at its left edge, then
        // the title area. Click in the title area to hit the tab, not the close button.
        let tabs_left = 3.0 * CW;
        let title_offset = CLOSE_CELLS * CW + CW; // past the left close button
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, tabs_left + title_offset, 5.0),
            ChromeHit::Tab(0)
        );
        // Second tab.
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, tabs_left + 18.0 * CW + title_offset, 5.0),
            ChromeHit::Tab(1)
        );
        // The new-tab button sits just past the last tab.
        assert_eq!(
            hit_test(
                &c,
                SURFACE_W,
                CW,
                CH,
                tabs_left + 2.0 * 18.0 * CW + 5.0,
                5.0
            ),
            ChromeHit::NewTab
        );
    }

    #[test]
    fn test_close_target_at_left_edge_of_tab() {
        let c = chrome(MenuStyle::Modern, 1, None);
        // Close button sits at the left edge of the first tab (right after the
        // hamburger), spanning CLOSE_CELLS * CW wide.
        let tab_left = 3.0 * CW; // hamburger width
        let inside_close = tab_left + CW; // within the 2-cell close region
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, inside_close, 5.0),
            ChromeHit::CloseTab(0)
        );
        // The far right of the tab is now the title area, not the close button.
        let right_of_tab = tab_left + 18.0 * CW - 5.0;
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, right_of_tab, 5.0),
            ChromeHit::Tab(0)
        );
    }

    #[test]
    fn test_hamburger_only_in_modern() {
        let modern = chrome(MenuStyle::Modern, 1, None);
        // The hamburger now sits at the left edge.
        assert_eq!(
            hit_test(&modern, SURFACE_W, CW, CH, 5.0, 5.0),
            ChromeHit::Hamburger
        );
        // Classic has menu titles on the top row instead.
        let classic = chrome(MenuStyle::Classic, 1, None);
        assert_eq!(
            hit_test(&classic, SURFACE_W, CW, CH, 2.0, 2.0),
            ChromeHit::MenuTitle(0)
        );
    }

    #[test]
    fn test_open_dropdown_items_are_hit_first() {
        let c = chrome(MenuStyle::Modern, 1, Some(0));
        let layout = layout(&c, SURFACE_W, CW, CH);
        let dropdown = layout.dropdown.expect("dropdown open");
        let first = dropdown_item_region(&dropdown, 0);
        let hit = hit_test(
            &c,
            SURFACE_W,
            CW,
            CH,
            first.x + 2.0,
            first.y + first.h / 2.0,
        );
        assert_eq!(hit, ChromeHit::DropdownItem(0));
        let second = dropdown_item_region(&dropdown, 1);
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, second.x + 2.0, second.y + 2.0),
            ChromeHit::DropdownItem(1)
        );
    }

    #[test]
    fn test_window_controls_hit_only_when_enabled() {
        let mut c = chrome(MenuStyle::Modern, 1, None);
        // Disabled by default: the far-right edge is empty title-bar space.
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, SURFACE_W - 5.0, 5.0),
            ChromeHit::None
        );

        c.window_controls = true;
        let w = CONTROL_CELLS * CW;
        // Rightmost is close, then maximize, then minimize moving left.
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, SURFACE_W - 1.0, 5.0),
            ChromeHit::Close
        );
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, SURFACE_W - w - 1.0, 5.0),
            ChromeHit::Maximize
        );
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, SURFACE_W - 2.0 * w - 1.0, 5.0),
            ChromeHit::Minimize
        );
    }

    #[test]
    fn test_click_in_empty_space_is_none() {
        let c = chrome(MenuStyle::Modern, 1, None);
        // Middle of the tabbar row, past the new-tab button, before the hamburger.
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, SURFACE_W / 2.0, 5.0),
            ChromeHit::None
        );
    }

    #[test]
    fn test_controls_left_puts_hamburger_right_and_mirrors_hits() {
        let mut c = chrome(MenuStyle::Modern, 1, None);
        c.window_controls = true;
        c.controls_side = ControlsSide::Left;
        let w = CONTROL_CELLS * CW;

        // Left edge is now Minimize (slot 0), then Maximize, then Close.
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, 1.0, 5.0),
            ChromeHit::Minimize
        );
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, w + 1.0, 5.0),
            ChromeHit::Maximize
        );
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, 2.0 * w + 1.0, 5.0),
            ChromeHit::Close
        );

        // Hamburger moved to the far right.
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, SURFACE_W - 1.0, 5.0),
            ChromeHit::Hamburger
        );
        let layout = layout(&c, SURFACE_W, CW, CH);
        let hamburger = layout.hamburger.expect("hamburger present");
        assert_eq!(hamburger.x + hamburger.w, SURFACE_W);

        // Tabs start after the three controls, not at the hamburger's old left slot.
        // Close button is now at the left of each tab, spanning CLOSE_CELLS * CW;
        // the title area begins after it.
        let tabs_left = 3.0 * w;
        let title_area = tabs_left + CLOSE_CELLS * CW + CW;
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, title_area, 5.0),
            ChromeHit::Tab(0)
        );
        // The first cell is no longer the hamburger (it became Minimize).
        assert_ne!(
            hit_test(&c, SURFACE_W, CW, CH, 1.0, 5.0),
            ChromeHit::Hamburger
        );
    }

    #[test]
    fn test_submenu_opens_right_of_parent_and_is_hit_first() {
        let mut c = chrome(MenuStyle::Modern, 1, Some(0));
        c.open_submenu = Some(1); // "Split" carries children.
        let layout = layout(&c, SURFACE_W, CW, CH);
        let parent = layout.dropdown.expect("parent open");
        let submenu = layout.submenu.expect("submenu open");
        // The child panel sits immediately right of the parent panel.
        assert_eq!(submenu.origin_x, parent.origin_x + parent.width);
        assert_eq!(submenu.items, 2);
        // A click on a child resolves to the submenu, which overlays the parent.
        let child = dropdown_item_region(&submenu, 0);
        assert_eq!(
            hit_test(
                &c,
                SURFACE_W,
                CW,
                CH,
                child.x + 2.0,
                child.y + child.h / 2.0
            ),
            ChromeHit::SubmenuItem(0)
        );
    }

    #[test]
    fn test_no_submenu_without_an_open_parent() {
        // open_submenu set but no menu open: no submenu geometry.
        let mut c = chrome(MenuStyle::Modern, 1, None);
        c.open_submenu = Some(1);
        assert!(layout(&c, SURFACE_W, CW, CH).submenu.is_none());
    }

    #[test]
    fn test_classic_dropdown_anchors_under_its_title() {
        let c = chrome(MenuStyle::Classic, 1, Some(1));
        let layout = layout(&c, SURFACE_W, CW, CH);
        let dropdown = layout.dropdown.expect("dropdown open");
        // Opens directly below the menubar row.
        assert_eq!(dropdown.top, CH);
        // Left edge aligns with the second menu title.
        assert_eq!(dropdown.origin_x, layout.menu_titles[1].x);
    }
}
