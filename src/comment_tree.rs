//! The comment thread's control: one interface over two implementations, chosen per platform.
//!
//! On Windows a native tree control cannot carry a whole comment. The common controls hand an item's text to a screen reader through a 260-character buffer, so a long comment is read up to that point and then simply stops, with nothing to say that more was there. There the thread is a plain focusable panel that describes itself to UI Automation through AccessKit instead, as a tree of items whose labels are as long as the comments are. Elsewhere `wxTreeCtrl` has no such limit and stays what it was, because a native control is still the better thing to be wherever it can say everything.
//!
//! Either way the control does not decide anything. Movement and opening or closing a thread are `App`'s, driven by the same key handlers as the story list; the control shows the result, and reports back only what the user did to it directly — a click, or a screen reader's own command — through the callbacks.

/// One comment as the tree needs it, already worded.
pub struct Row {
    pub label: String,
    /// 0-based, as `CommentRow::depth`.
    pub depth: usize,
    /// A native tree works this out from the nesting it was given.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub has_replies: bool,
    pub collapsed: bool,
}

#[cfg(windows)]
pub use accessible::CommentTree;
#[cfg(not(windows))]
pub use native::CommentTree;

#[cfg(windows)]
mod accessible {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::mpsc;

    use accesskit::{Action, ActionHandler, ActionRequest, ActivationHandler, Node, NodeId, Role, TreeId, TreeInfo, TreeUpdate};
    use accesskit_windows::{HWND, SubclassingAdapter};
    use wxdragon::event::WindowEvents;
    use wxdragon::prelude::*;

    use super::Row;

    const ROOT: NodeId = NodeId(0);

    /// `wxWANTS_CHARS`, which wxdragon has no name for. Without it the panel's parent treats arrows, Tab and Enter as dialog navigation and they never arrive here as keys.
    const WANTS_CHARS: i64 = 0x0004_0000;

    fn node_id(index: usize) -> NodeId {
        NodeId(index as u64 + 1)
    }

    fn row_index(id: NodeId) -> Option<usize> {
        id.0.checked_sub(1).map(|index| index as usize)
    }

    /// The thread as AccessKit sees it: the rows, the nesting their depths describe, and which one is selected.
    #[derive(Default)]
    struct Shape {
        rows: Vec<Row>,
        parent: Vec<Option<usize>>,
        replies: Vec<Vec<usize>>,
        top: Vec<usize>,
        /// Each row's place among its siblings, which is what "3 of 5" counts.
        position: Vec<usize>,
        cursor: usize,
    }

    impl Shape {
        fn new(rows: Vec<Row>, cursor: usize) -> Self {
            let count = rows.len();
            let mut shape = Shape {
                parent: vec![None; count],
                replies: vec![Vec::new(); count],
                top: Vec::new(),
                position: vec![0; count],
                rows,
                cursor,
            };
            // The thread arrives flattened into reading order with a depth on each row; the stack turns that back into nesting.
            let mut stack: Vec<usize> = Vec::new();
            for index in 0..count {
                let depth = shape.rows[index].depth;
                while stack.last().is_some_and(|&above| shape.rows[above].depth >= depth) {
                    stack.pop();
                }
                let siblings = match stack.last() {
                    Some(&parent) => {
                        shape.parent[index] = Some(parent);
                        &mut shape.replies[parent]
                    }
                    None => &mut shape.top,
                };
                shape.position[index] = siblings.len();
                siblings.push(index);
                stack.push(index);
            }
            shape
        }

        fn focus(&self) -> NodeId {
            if self.rows.is_empty() { ROOT } else { node_id(self.cursor) }
        }

        /// Whether a row is in the tree at all: a comment inside a closed thread is not hidden but absent, as it is in a native tree.
        fn is_visible(&self, index: usize) -> bool {
            let mut parent = self.parent.get(index).copied().flatten();
            while let Some(above) = parent {
                if self.rows[above].collapsed {
                    return false;
                }
                parent = self.parent[above];
            }
            index < self.rows.len()
        }

        fn root(&self) -> Node {
            let mut root = Node::new(Role::Tree);
            root.set_children(self.top.iter().map(|&index| node_id(index)).collect::<Vec<_>>());
            root.set_size_of_set(self.top.len());
            root
        }

        fn node(&self, index: usize) -> Node {
            let row = &self.rows[index];
            let mut node = Node::new(Role::TreeItem);
            node.set_label(row.label.as_str());
            node.set_level(row.depth);
            node.set_position_in_set(self.position[index]);
            node.add_action(Action::Focus);
            if row.has_replies {
                node.set_expanded(!row.collapsed);
                if row.collapsed {
                    node.add_action(Action::Expand);
                } else {
                    let replies = &self.replies[index];
                    node.add_action(Action::Collapse);
                    node.set_children(replies.iter().map(|&reply| node_id(reply)).collect::<Vec<_>>());
                    // AccessKit counts a set from its container, so the size of the replies' set lives on the comment they reply to.
                    node.set_size_of_set(replies.len());
                }
            }
            if index == self.cursor {
                node.set_selected(true);
            }
            node
        }

        /// Every node a screen reader can reach. AccessKit refuses a node that no parent in the update claims, so a closed thread's replies are left out rather than sent and orphaned.
        fn visible_nodes(&self) -> Vec<(NodeId, Node)> {
            let mut nodes = vec![(ROOT, self.root())];
            let mut pending: Vec<usize> = self.top.iter().rev().copied().collect();
            while let Some(index) = pending.pop() {
                nodes.push((node_id(index), self.node(index)));
                if !self.rows[index].collapsed {
                    pending.extend(self.replies[index].iter().rev());
                }
            }
            nodes
        }

        fn full_update(&self, initial: bool) -> TreeUpdate {
            TreeUpdate {
                nodes: self.visible_nodes(),
                tree: initial.then(|| TreeInfo::new(ROOT)),
                tree_id: TreeId::ROOT,
                focus: self.focus(),
            }
        }
    }

    struct Activation(Rc<RefCell<Shape>>);

    impl ActivationHandler for Activation {
        fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
            Some(self.0.borrow().full_update(true))
        }
    }

    /// A screen reader's request, which UI Automation may deliver on any thread. It goes back to the GUI thread the same way the network worker's results do.
    struct Actions(mpsc::Sender<ActionRequest>);

    impl ActionHandler for Actions {
        fn do_action(&mut self, request: ActionRequest) {
            if self.0.send(request).is_ok() {
                wxdragon::wake_up_idle();
            }
        }
    }

    type Callback<T> = RefCell<Option<Box<dyn Fn(T)>>>;

    struct Inner {
        adapter: RefCell<SubclassingAdapter>,
        shape: Rc<RefCell<Shape>>,
        actions: mpsc::Receiver<ActionRequest>,
        on_select: Callback<usize>,
        on_toggle: Callback<(usize, bool)>,
    }

    #[derive(Clone)]
    pub struct CommentTree {
        panel: Panel,
        inner: Rc<Inner>,
    }

    impl CommentTree {
        /// Must run before `parent` is first shown: AccessKit can only take over a window that has never been visible.
        pub fn new(parent: &impl WxWidget) -> Self {
            let panel = Panel::builder(parent).with_style(PanelStyle::TabTraversal).build();
            // Empty, so the window itself has nothing of its own to say over the tree it hosts.
            panel.set_name("");
            panel.set_label("");
            panel.set_style_raw(panel.get_style_raw() | WANTS_CHARS);
            panel.set_can_focus(true);
            panel.set_extra_style(ExtraWindowStyle::ProcessIdle);

            let shape = Rc::new(RefCell::new(Shape::default()));
            let (sender, actions) = mpsc::channel();
            let adapter = SubclassingAdapter::new(
                HWND(panel.get_handle()),
                Activation(shape.clone()),
                Actions(sender),
            );

            let tree = CommentTree {
                panel,
                inner: Rc::new(Inner {
                    adapter: RefCell::new(adapter),
                    shape,
                    actions,
                    on_select: RefCell::new(None),
                    on_toggle: RefCell::new(None),
                }),
            };

            let t = tree.clone();
            tree.panel.on_idle(move |_| t.handle_actions());
            // Nothing is drawn to click on, but a click should at least put the keyboard here.
            let t = tree.clone();
            tree.panel.on_mouse_left_down(move |e: WindowEventData| {
                t.panel.set_focus();
                e.skip(true);
            });
            tree
        }

        /// What keys and the context menu are bound to.
        pub fn widget(&self) -> Panel {
            self.panel
        }

        pub fn show(&self, show: bool) {
            self.panel.show(show);
        }

        /// Replace the whole thread.
        pub fn set_rows(&self, rows: Vec<Row>, cursor: usize) {
            *self.inner.shape.borrow_mut() = Shape::new(rows, cursor);
            self.send_full_update();
            self.take_focus();
        }

        pub fn clear(&self) {
            self.set_rows(Vec::new(), 0);
        }

        /// Move the selection. Only the two rows whose selected state changed are sent: anything AccessKit sees change on the focused item, it tells the screen reader about.
        pub fn select(&self, index: usize) {
            {
                let shape = self.inner.shape.borrow();
                if index >= shape.rows.len() {
                    return;
                }
            }
            let old = std::mem::replace(&mut self.inner.shape.borrow_mut().cursor, index);
            let update = {
                let shape = self.inner.shape.borrow();
                let mut nodes = Vec::with_capacity(2);
                if old != index && shape.is_visible(old) {
                    nodes.push((node_id(old), shape.node(old)));
                }
                if shape.is_visible(index) {
                    nodes.push((node_id(index), shape.node(index)));
                }
                TreeUpdate {
                    nodes,
                    tree: None,
                    tree_id: TreeId::ROOT,
                    focus: shape.focus(),
                }
            };
            self.send(update);
            self.take_focus();
        }

        /// Open or close one comment's replies; `cursor` is where `App` put the selection afterwards, which closing a thread around it can have moved.
        pub fn set_collapsed(&self, index: usize, collapsed: bool, cursor: usize) {
            {
                let mut shape = self.inner.shape.borrow_mut();
                let Some(row) = shape.rows.get_mut(index) else {
                    return;
                };
                row.collapsed = collapsed;
                shape.cursor = cursor;
            }
            self.send_full_update();
        }

        /// A screen reader moved to a comment itself, rather than through the keys this application handles.
        pub fn on_select(&self, callback: impl Fn(usize) + 'static) {
            *self.inner.on_select.borrow_mut() = Some(Box::new(callback));
        }

        /// A screen reader asked to open (`false`) or close (`true`) a comment's replies. The tree is left as it was until the application calls `set_collapsed`.
        pub fn on_toggle(&self, callback: impl Fn(usize, bool) + 'static) {
            *self.inner.on_toggle.borrow_mut() = Some(Box::new(move |(index, collapsed)| callback(index, collapsed)));
        }

        /// Enter already reaches the key handlers, and there is nothing to double-click, so there is no other way to activate an item here.
        pub fn on_activate(&self, _callback: impl Fn() + 'static) {}

        fn send_full_update(&self) {
            let update = self.inner.shape.borrow().full_update(false);
            self.send(update);
        }

        fn send(&self, update: TreeUpdate) {
            // The update is built before the adapter is touched, so no borrow of `shape` is held if it turns around and asks for the initial tree.
            let events = self.inner.adapter.borrow_mut().update_if_active(|| update);
            if let Some(events) = events {
                events.raise();
            }
        }

        fn take_focus(&self) {
            if self.panel.is_shown() && !self.panel.has_focus() {
                self.panel.set_focus();
            }
        }

        fn handle_actions(&self) {
            while let Ok(request) = self.inner.actions.try_recv() {
                let Some(index) = row_index(request.target_node) else {
                    continue;
                };
                // A request can be overtaken by the thread changing under it; one for a row no longer in the tree is about something that is gone.
                let row_collapsed = {
                    let shape = self.inner.shape.borrow();
                    if !shape.is_visible(index) {
                        continue;
                    }
                    shape.rows[index].collapsed
                };
                match request.action {
                    Action::Focus => {
                        if let Some(callback) = self.inner.on_select.borrow().as_ref() {
                            callback(index);
                        }
                    }
                    Action::Expand | Action::Collapse => {
                        let collapsed = request.action == Action::Collapse;
                        if collapsed != row_collapsed
                            && let Some(callback) = self.inner.on_toggle.borrow().as_ref()
                        {
                            callback((index, collapsed));
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

#[cfg(not(windows))]
mod native {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use wxdragon::event::TreeEventData;
    use wxdragon::prelude::*;
    use wxdragon::widgets::item_data::HasItemData;

    use super::Row;

    #[derive(Clone)]
    pub struct CommentTree {
        tree: TreeCtrl,
        /// The tree node for each row, so a cursor index can be turned into something `TreeCtrl` understands.
        items: Rc<RefCell<Vec<TreeItemId>>>,
        /// Set while this application is changing the tree itself, so the events that come back from it (some platforms fire them synchronously) are not mistaken for something the user did.
        suppress: Rc<Cell<bool>>,
    }

    impl CommentTree {
        pub fn new(parent: &impl WxWidget) -> Self {
            let tree = TreeCtrl::builder(parent)
                .with_style(TreeCtrlStyle::HasButtons | TreeCtrlStyle::LinesAtRoot | TreeCtrlStyle::HideRoot)
                .build();
            CommentTree {
                tree,
                items: Rc::new(RefCell::new(Vec::new())),
                suppress: Rc::new(Cell::new(false)),
            }
        }

        pub fn widget(&self) -> TreeCtrl {
            self.tree
        }

        pub fn show(&self, show: bool) {
            self.tree.show(show);
        }

        pub fn set_rows(&self, rows: Vec<Row>, cursor: usize) {
            self.suppress.set(true);
            self.tree.delete_all_items();
            let mut items: Vec<TreeItemId> = Vec::new();
            if !rows.is_empty()
                && let Some(root) = self.tree.add_root("Comments", None, None)
            {
                // The thread arrives flattened into reading order with a depth on each row; `stack` turns that back into nesting, which is what makes the tree announce reply level.
                let mut stack: Vec<(usize, TreeItemId)> = Vec::new();
                for (i, row) in rows.iter().enumerate() {
                    while stack.last().is_some_and(|(d, _)| *d >= row.depth) {
                        stack.pop();
                    }
                    let parent = stack.last().map(|(_, id)| id).unwrap_or(&root);
                    if let Some(item) = self.tree.append_item_with_data(parent, &row.label, i, None, None) {
                        items.push(item.clone());
                        stack.push((row.depth, item));
                    }
                }
                // Expand first so every reply has been laid out, then close the threads the user had closed: `collapse` on an item that was never expanded is a no-op, so the order matters when a view is rebuilt.
                self.tree.expand_all();
                for (item, row) in items.iter().zip(&rows) {
                    if row.collapsed {
                        self.tree.collapse(item);
                    }
                }
            }
            *self.items.borrow_mut() = items;
            self.suppress.set(false);
            self.select(cursor);
        }

        /// Hiding the tree does not empty it, so without this a thread the user has left stays in memory -- every native node with its label, plus our handles into them -- for as long as they browse elsewhere.
        pub fn clear(&self) {
            self.suppress.set(true);
            self.tree.delete_all_items();
            self.items.borrow_mut().clear();
            self.suppress.set(false);
        }

        /// Point the tree at one row. `index` is an index into the whole thread, so it is only ever a row that is actually showing — `ensure_visible` would otherwise reopen the very thread the user had just collapsed.
        ///
        /// No `unselect_all` first: this is a single-selection tree, so selecting is already a replacement, and the moment with nothing selected that clearing leaves behind is a moment a screen reader has to say something about.
        pub fn select(&self, index: usize) {
            self.suppress.set(true);
            if let Some(item) = self.items.borrow().get(index) {
                self.tree.select_item(item);
                self.tree.ensure_visible(item);
                self.tree.set_focus();
            }
            self.suppress.set(false);
        }

        pub fn set_collapsed(&self, index: usize, collapsed: bool, cursor: usize) {
            self.suppress.set(true);
            if let Some(item) = self.items.borrow().get(index) {
                if collapsed {
                    self.tree.collapse(item);
                } else {
                    self.tree.expand(item);
                }
            }
            self.suppress.set(false);
            self.select(cursor);
        }

        pub fn on_select(&self, callback: impl Fn(usize) + 'static) {
            let t = self.clone();
            self.tree.on_selection_changed(move |e: TreeEventData| {
                if let Some(index) = t.event_row(&e) {
                    callback(index);
                }
            });
        }

        /// Clicking the expander is the other way to open and close a thread, and the cursor has to step over the same rows afterwards however it was done.
        pub fn on_toggle(&self, callback: impl Fn(usize, bool) + 'static) {
            let callback = Rc::new(callback);
            for collapsed in [true, false] {
                let t = self.clone();
                let callback = callback.clone();
                let handler = move |e: TreeEventData| {
                    if let Some(index) = t.event_row(&e) {
                        callback(index, collapsed);
                    }
                };
                if collapsed {
                    self.tree.on_item_collapsed(handler);
                } else {
                    self.tree.on_item_expanded(handler);
                }
            }
        }

        pub fn on_activate(&self, callback: impl Fn() + 'static) {
            self.tree.on_item_activated(move |_| callback());
        }

        /// The thread row an event's item stands for, unless the event is an echo of this application's own change. Every item carries its index into the thread as its data, which is the only thing that survives the tree being rebuilt.
        fn event_row(&self, e: &TreeEventData) -> Option<usize> {
            if self.suppress.get() {
                return None;
            }
            let item = e.get_item()?;
            self.tree.get_custom_data(&item).and_then(|data| data.downcast_ref::<usize>().copied())
        }
    }
}
