//! VoiceOver bridge: turns GPUI's AccessKit tree into `UIAccessibilityElement`s.
//!
//! UIKit pulls accessibility from views, so `GPUIView` acts as an
//! accessibility container (`accessibilityElements`) whose children are
//! one element per actionable or labelled AccessKit node, in tree order.
//! GPUI only builds the tree while the adapter is active, so the bridge
//! activates it when VoiceOver (or another assistive tech) is running and
//! deactivates it when it stops.
//!
//! Buttons merge their descendant text into one label so VoiceOver reads
//! "New session, button" instead of walking into the glyphs. Activating an
//! element (double tap) sends `Action::Click` back to GPUI, which runs the
//! element's click listeners.

use crate::{CGPoint, CGRect, CGSize, id, nil, ns_string};
use accesskit::{Action, ActionRequest, Node, NodeId, Role, TreeId, TreeUpdate};
use gpui::A11yCallbacks;
use objc::{
    class,
    declare::ClassDecl,
    msg_send,
    runtime::{BOOL, Class, NO, Object, Sel, YES},
    sel, sel_impl,
};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    ffi::c_void,
    sync::OnceLock,
};

#[link(name = "UIKit", kind = "framework")]
unsafe extern "C" {
    fn UIAccessibilityIsVoiceOverRunning() -> BOOL;
    fn UIAccessibilityIsSwitchControlRunning() -> BOOL;
    fn UIAccessibilityPostNotification(notification: u32, argument: id);
    static UIAccessibilityTraitButton: u64;
    static UIAccessibilityTraitStaticText: u64;
    static UIAccessibilityTraitHeader: u64;
    static UIAccessibilityTraitSelected: u64;
    static UIAccessibilityTraitNotEnabled: u64;
}

const LAYOUT_CHANGED: u32 = 1001;
const NODE_IVAR: &str = "gpuiNodeId";
const BRIDGE_IVAR: &str = "gpuiBridge";

/// One VoiceOver-visible element derived from the AccessKit tree.
#[derive(Clone, PartialEq)]
struct Item {
    node: NodeId,
    label: String,
    value: String,
    frame: (f64, f64, f64, f64),
    traits: u64,
}

pub(crate) struct A11yBridge {
    callbacks: RefCell<Option<A11yCallbacks>>,
    active: Cell<bool>,
    nodes: RefCell<HashMap<NodeId, Node>>,
    root: Cell<Option<NodeId>>,
    tree: Cell<TreeId>,
    /// `UIAccessibilityElement`s by node, reused across frames (retained).
    elements: RefCell<HashMap<NodeId, id>>,
    /// Current ordered element list (retained NSArray) and its source items.
    array: Cell<id>,
    items: RefCell<Vec<Item>>,
    scale: Cell<f64>,
}

impl A11yBridge {
    pub(crate) fn new() -> Self {
        Self {
            callbacks: RefCell::new(None),
            active: Cell::new(false),
            nodes: RefCell::new(HashMap::new()),
            root: Cell::new(None),
            tree: Cell::new(TreeId::ROOT),
            elements: RefCell::new(HashMap::new()),
            array: Cell::new(nil),
            items: RefCell::new(Vec::new()),
            scale: Cell::new(1.0),
        }
    }

    pub(crate) fn init(&self, callbacks: A11yCallbacks, view: id) {
        *self.callbacks.borrow_mut() = Some(callbacks);
        unsafe {
            let center: id = msg_send![class!(NSNotificationCenter), defaultCenter];
            for name in [
                "UIAccessibilityVoiceOverStatusDidChangeNotification",
                "UIAccessibilitySwitchControlStatusDidChangeNotification",
            ] {
                let _: () = msg_send![center, addObserver: view selector: sel!(gpuiA11yStatusChanged:) name: ns_string(name) object: nil];
            }
        }
        self.sync_status();
    }

    /// Activates GPUI's tree when assistive tech is running, deactivates it otherwise.
    pub(crate) fn sync_status(&self) {
        let wanted = unsafe { UIAccessibilityIsVoiceOverRunning() == YES || UIAccessibilityIsSwitchControlRunning() == YES }
            || std::env::var_os("GPUI_IOS_A11Y").is_some();
        if wanted == self.active.get() {
            return;
        }
        self.active.set(wanted);
        log::info!("ios a11y {}", if wanted { "activated" } else { "deactivated" });
        let callbacks = self.callbacks.borrow();
        let Some(cb) = callbacks.as_ref() else { return };
        if wanted {
            if let Some(initial) = (cb.activation)() {
                drop(callbacks);
                self.apply(initial);
            }
        } else {
            (cb.deactivation)();
        }
    }

    pub(crate) fn set_scale(&self, scale: f64) {
        self.scale.set(scale.max(1.0));
    }

    pub(crate) fn apply(&self, update: TreeUpdate) {
        if let Some(tree) = &update.tree {
            self.root.set(Some(tree.root));
            self.nodes.borrow_mut().clear();
        }
        self.tree.set(update.tree_id);
        {
            let mut nodes = self.nodes.borrow_mut();
            for (node_id, node) in update.nodes {
                nodes.insert(node_id, node);
            }
        }
        let items = self.flatten();
        if *self.items.borrow() != items {
            self.rebuild(&items);
            *self.items.borrow_mut() = items;
            unsafe { UIAccessibilityPostNotification(LAYOUT_CHANGED, nil) };
        }
    }

    fn flatten(&self) -> Vec<Item> {
        let nodes = self.nodes.borrow();
        let mut out = Vec::new();
        if let Some(root) = self.root.get() {
            let mut seen = std::collections::HashSet::new();
            for child in nodes.get(&root).map(|n| n.children().to_vec()).unwrap_or_default() {
                walk(&nodes, child, self.scale.get(), &mut out, &mut seen);
            }
        }
        out
    }

    fn rebuild(&self, items: &[Item]) {
        let mut elements = self.elements.borrow_mut();
        let live: std::collections::HashSet<NodeId> = items.iter().map(|i| i.node).collect();
        elements.retain(|node, el| {
            let keep = live.contains(node);
            if !keep {
                unsafe {
                    (**el).set_ivar::<*const c_void>(BRIDGE_IVAR, std::ptr::null());
                    let _: () = msg_send![*el, release];
                }
            }
            keep
        });
        unsafe {
            let view = VIEW.with(|v| v.get());
            let list: id = msg_send![class!(NSMutableArray), arrayWithCapacity: items.len()];
            for item in items {
                let el = *elements.entry(item.node).or_insert_with(|| {
                    let el: id = msg_send![element_class(), alloc];
                    let el: id = msg_send![el, initWithAccessibilityContainer: view];
                    (*el).set_ivar::<u64>(NODE_IVAR, item.node.0);
                    el
                });
                (*el).set_ivar::<*const c_void>(BRIDGE_IVAR, self as *const Self as *const c_void);
                let _: () = msg_send![el, setIsAccessibilityElement: YES];
                let _: () = msg_send![el, setAccessibilityLabel: ns_string(&item.label)];
                let value = if item.value.is_empty() { nil } else { ns_string(&item.value) };
                let _: () = msg_send![el, setAccessibilityValue: value];
                let _: () = msg_send![el, setAccessibilityTraits: item.traits];
                let (x, y, w, h) = item.frame;
                let frame = CGRect { origin: CGPoint { x, y }, size: CGSize { width: w, height: h } };
                // Frames are relative to the container so they follow the view.
                let _: () = msg_send![el, setAccessibilityFrameInContainerSpace: frame];
                let _: () = msg_send![list, addObject: el];
            }
            let old = self.array.replace(msg_send![list, copy]);
            if !old.is_null() {
                let _: () = msg_send![old, release];
            }
        }
    }

    fn activate(&self, node: NodeId) -> bool {
        let callbacks = self.callbacks.borrow();
        let Some(cb) = callbacks.as_ref() else { return false };
        (cb.action)(ActionRequest {
            action: Action::Click,
            target_tree: self.tree.get(),
            target_node: node,
            data: None,
        });
        true
    }

    pub(crate) fn elements(&self) -> id {
        if self.active.get() { self.array.get() } else { nil }
    }
}

fn text_of(node: &Node) -> String {
    node.label()
        .map(str::to_string)
        .or_else(|| node.value().map(str::to_string))
        .unwrap_or_default()
}

/// Concatenated text of a subtree, for buttons without an explicit label.
fn subtree_text(nodes: &HashMap<NodeId, Node>, id: NodeId, out: &mut Vec<String>, depth: usize) {
    let Some(node) = nodes.get(&id) else { return };
    if depth > 64 || node.is_hidden() {
        return;
    }
    let t = text_of(node);
    if !t.trim().is_empty() {
        out.push(t.trim().to_string());
        return;
    }
    for c in node.children() {
        subtree_text(nodes, *c, out, depth + 1);
    }
}

fn walk(
    nodes: &HashMap<NodeId, Node>,
    id: NodeId,
    scale: f64,
    out: &mut Vec<Item>,
    seen: &mut std::collections::HashSet<NodeId>,
) {
    let Some(node) = nodes.get(&id) else { return };
    if !seen.insert(id) || node.is_hidden() {
        return;
    }
    let clickable = node.supports_action(Action::Click);
    let role = node.role();
    let is_button = clickable
        || matches!(role, Role::Button | Role::Link | Role::CheckBox | Role::Switch | Role::Tab | Role::MenuItem | Role::RadioButton);
    let mut label = node.label().unwrap_or_default().to_string();
    let value = if node.label().is_some() { node.value().unwrap_or_default().to_string() } else { String::new() };
    if is_button && label.trim().is_empty() {
        let mut parts = Vec::new();
        for c in node.children() {
            subtree_text(nodes, *c, &mut parts, 0);
        }
        label = parts.join(" ");
    } else if label.trim().is_empty() {
        label = node.value().unwrap_or_default().to_string();
    }
    let text_leaf = !is_button && !label.trim().is_empty();
    if is_button || text_leaf {
        if let Some(b) = node.bounds() {
            let mut traits = unsafe {
                if is_button {
                    UIAccessibilityTraitButton
                } else if role == Role::Heading {
                    UIAccessibilityTraitHeader
                } else {
                    UIAccessibilityTraitStaticText
                }
            };
            if node.is_selected() == Some(true) || matches!(node.toggled(), Some(accesskit::Toggled::True)) {
                traits |= unsafe { UIAccessibilityTraitSelected };
            }
            if node.is_disabled() {
                traits |= unsafe { UIAccessibilityTraitNotEnabled };
            }
            if b.x1 > b.x0 && b.y1 > b.y0 && !(is_button && label.trim().is_empty()) {
                out.push(Item {
                    node: id,
                    label: label.trim().to_string(),
                    value,
                    frame: (b.x0 / scale, b.y0 / scale, (b.x1 - b.x0) / scale, (b.y1 - b.y0) / scale),
                    traits,
                });
            }
        }
        // A button speaks for its contents.
        if is_button {
            return;
        }
    }
    for c in node.children() {
        walk(nodes, *c, scale, out, seen);
    }
}

thread_local! {
    static VIEW: Cell<id> = const { Cell::new(std::ptr::null_mut()) };
}

pub(crate) fn set_view(view: id) {
    VIEW.with(|v| v.set(view));
}

fn element_class() -> &'static Class {
    static CLASS: OnceLock<usize> = OnceLock::new();
    let ptr = *CLASS.get_or_init(|| unsafe {
        let mut decl = ClassDecl::new("GPUIAccessibilityElement", class!(UIAccessibilityElement)).expect("a11y element");
        decl.add_ivar::<u64>(NODE_IVAR);
        decl.add_ivar::<*const c_void>(BRIDGE_IVAR);
        decl.add_method(sel!(accessibilityActivate), element_activate as extern "C" fn(&Object, Sel) -> BOOL);
        decl.register() as *const Class as usize
    });
    unsafe { &*(ptr as *const Class) }
}

extern "C" fn element_activate(this: &Object, _: Sel) -> BOOL {
    unsafe {
        let bridge: *const c_void = *this.get_ivar(BRIDGE_IVAR);
        let node: u64 = *this.get_ivar(NODE_IVAR);
        match (bridge as *const A11yBridge).as_ref() {
            Some(b) if b.activate(NodeId(node)) => YES,
            _ => NO,
        }
    }
}
