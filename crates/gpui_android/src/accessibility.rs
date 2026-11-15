//! AccessKit semantics for Android's service-only virtual accessibility provider.
//!
//! All GPUI callbacks execute in `process_events`, on the native GPUI event loop.
//! Java only owns immutable snapshots and queues requests, never application UI.
use accesskit::{
    Action, ActionData, ActionRequest, Affine, Node, NodeId, Role, TreeId, TreeUpdate,
};
use gpui::A11yCallbacks;
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

const MAX_EVENTS: usize = 256;
const MAX_NODES: usize = 16_384;
static NEXT_BRIDGE: AtomicU64 = AtomicU64::new(1);
static EVENTS: Mutex<VecDeque<(u64, Event)>> = Mutex::new(VecDeque::new());
#[cfg(target_os = "android")]
static WAKER: Mutex<Option<android_activity::AndroidAppWaker>> = Mutex::new(None);

#[derive(Debug)]
enum Event {
    Status(bool),
    Action {
        virtual_id: i32,
        action: i32,
        value: Option<String>,
    },
}

fn enqueue(bridge: u64, event: Event) -> bool {
    let mut queue = EVENTS.lock().unwrap_or_else(|error| error.into_inner());
    if queue.len() >= MAX_EVENTS {
        return false;
    }
    queue.push_back((bridge, event));
    drop(queue);
    #[cfg(target_os = "android")]
    if let Some(waker) = WAKER
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
    {
        waker.wake();
    }
    true
}

#[cfg(target_os = "android")]
pub(crate) fn initialize(waker: android_activity::AndroidAppWaker) {
    *WAKER.lock().unwrap_or_else(|error| error.into_inner()) = Some(waker);
}

#[derive(Clone, Debug, PartialEq)]
struct Item {
    id: i32,
    node: NodeId,
    label: String,
    value: String,
    class_name: &'static str,
    bounds: [f64; 4],
    enabled: bool,
    selected: bool,
    checked: Option<bool>,
    password: bool,
    heading: bool,
    editable: bool,
    actions: u32,
}

pub(crate) struct A11yBridge {
    token: u64,
    callbacks: RefCell<Option<A11yCallbacks>>,
    active: Cell<bool>,
    nodes: RefCell<HashMap<NodeId, Node>>,
    root: Cell<Option<NodeId>>,
    tree: Cell<TreeId>,
    focus: Cell<Option<NodeId>>,
    ids: RefCell<HashMap<NodeId, i32>>,
    next_id: Cell<i32>,
    items: RefCell<Vec<Item>>,
    scale: Cell<f64>,
    last_snapshot: RefCell<String>,
}

impl A11yBridge {
    pub(crate) fn new() -> Self {
        Self {
            token: NEXT_BRIDGE.fetch_add(1, Ordering::Relaxed),
            callbacks: RefCell::new(None),
            active: Cell::new(false),
            nodes: RefCell::new(HashMap::new()),
            root: Cell::new(None),
            tree: Cell::new(TreeId::ROOT),
            focus: Cell::new(None),
            ids: RefCell::new(HashMap::new()),
            next_id: Cell::new(1),
            items: RefCell::new(Vec::new()),
            scale: Cell::new(1.0),
            last_snapshot: RefCell::new(String::new()),
        }
    }

    pub(crate) fn init(&self, callbacks: A11yCallbacks) {
        *self.callbacks.borrow_mut() = Some(callbacks);
        #[cfg(target_os = "android")]
        java_call(self.token, None, false);
    }

    /// AccessKit bounds are physical pixels. A caller with logical bounds may
    /// supply the logical-to-physical multiplier here. Android does not use dp.
    pub(crate) fn set_scale(&self, scale: f64) {
        if scale.is_finite() && scale > 0.0 {
            self.scale.set(scale);
        }
    }

    pub(crate) fn process_events(&self) {
        let pending = {
            let mut queue = EVENTS.lock().unwrap_or_else(|error| error.into_inner());
            let mut pending = Vec::new();
            let mut remaining = VecDeque::new();
            while let Some((token, event)) = queue.pop_front() {
                if token == self.token {
                    pending.push(event);
                } else {
                    remaining.push_back((token, event));
                }
            }
            *queue = remaining;
            pending
        };
        for event in pending {
            match event {
                Event::Status(wanted) => {
                    if self.active.replace(wanted) == wanted {
                        continue;
                    }
                    let initial = self.callbacks.borrow().as_ref().and_then(|callbacks| {
                        if wanted {
                            (callbacks.activation)()
                        } else {
                            (callbacks.deactivation)();
                            None
                        }
                    });
                    if let Some(update) = initial {
                        self.apply(update);
                    }
                    if !wanted {
                        self.items.borrow_mut().clear();
                        self.nodes.borrow_mut().clear();
                        self.ids.borrow_mut().clear();
                        self.root.set(None);
                        self.publish();
                    }
                }
                Event::Action {
                    virtual_id,
                    action,
                    value,
                } => {
                    if !self.active.get() {
                        continue;
                    }
                    if let Some(request) = self.action_request(virtual_id, action, value) {
                        if let Some(callbacks) = self.callbacks.borrow().as_ref() {
                            (callbacks.action)(request);
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn apply(&self, update: TreeUpdate) {
        // Embedded trees have independent NodeId spaces. Until graft support
        // exists, never merge a subtree into the root or misroute its actions.
        if update.tree_id != TreeId::ROOT {
            return;
        }
        if let Some(tree) = update.tree {
            if self.root.get() != Some(tree.root) || self.tree.get() != update.tree_id {
                self.nodes.borrow_mut().clear();
                // Never reuse Android virtual IDs across tree replacements.
                self.ids.borrow_mut().clear();
            }
            self.root.set(Some(tree.root));
        }
        self.tree.set(update.tree_id);
        self.focus.set(Some(update.focus));
        {
            let mut nodes = self.nodes.borrow_mut();
            for (id, node) in update.nodes {
                nodes.insert(id, node);
            }
        }
        let next = self.flatten();
        *self.items.borrow_mut() = next;
        self.publish();
    }

    fn flatten(&self) -> Vec<Item> {
        let mut out = Vec::new();
        let nodes = self.nodes.borrow();
        let mut seen = HashSet::new();
        if let Some(root) = self.root.get() {
            self.walk(
                &nodes,
                root,
                Affine::IDENTITY,
                false,
                &mut seen,
                &mut out,
                0,
            );
        }
        // Only reachable nodes are actionable, and detached nodes cannot grow
        // the cache forever through incremental updates.
        drop(nodes);
        self.nodes.borrow_mut().retain(|id, _| seen.contains(id));
        let visible: HashSet<_> = out.iter().map(|item| item.node).collect();
        self.ids.borrow_mut().retain(|id, _| visible.contains(id));
        out
    }

    fn walk(
        &self,
        nodes: &HashMap<NodeId, Node>,
        id: NodeId,
        parent: Affine,
        ancestor_disabled: bool,
        seen: &mut HashSet<NodeId>,
        out: &mut Vec<Item>,
        depth: usize,
    ) {
        if depth > 128 || seen.len() >= MAX_NODES || !seen.insert(id) {
            return;
        }
        let Some(node) = nodes.get(&id) else { return };
        if node.is_hidden() {
            mark_descendants(nodes, id, seen, depth);
            return;
        }
        let transform = parent * node.transform().copied().unwrap_or(Affine::IDENTITY);
        let enabled = !ancestor_disabled && !node.is_disabled();
        let actions = action_mask(node);
        let editable = is_text_input(node.role());
        let merge = node.supports_action(Action::Click) && !editable;
        let mut label = node.label().unwrap_or_default().trim().to_owned();
        if merge && label.is_empty() {
            let mut parts = Vec::new();
            subtree_text(nodes, id, &mut HashSet::new(), &mut parts, 0);
            label = parts.join(" ");
        }
        let password = node.role() == Role::PasswordInput;
        let value = if password {
            String::new()
        } else {
            node.value().unwrap_or_default().to_owned()
        };
        let semantic = id != self.root.get().unwrap_or(id)
            && (!label.is_empty() || !value.is_empty() || actions != 0 || editable);
        if semantic {
            if let Some(bounds) = node.bounds() {
                let bounds = transform.transform_rect_bbox(bounds);
                let scale = self.scale.get();
                let bounds = [
                    bounds.x0 * scale,
                    bounds.y0 * scale,
                    bounds.x1 * scale,
                    bounds.y1 * scale,
                ];
                if bounds.iter().all(|n| n.is_finite())
                    && bounds[2] > bounds[0]
                    && bounds[3] > bounds[1]
                {
                    let next = self.next_id.get();
                    if next == i32::MAX {
                        return;
                    }
                    let virtual_id = *self.ids.borrow_mut().entry(id).or_insert_with(|| {
                        self.next_id.set(next + 1);
                        next
                    });
                    out.push(Item {
                        id: virtual_id,
                        node: id,
                        label,
                        value,
                        class_name: class_name(node.role()),
                        bounds,
                        enabled,
                        selected: node.is_selected() == Some(true),
                        checked: node
                            .toggled()
                            .map(|value| value == accesskit::Toggled::True),
                        password,
                        heading: node.role() == Role::Heading,
                        editable,
                        actions,
                    });
                    if merge {
                        // Keep descendants reachable for subsequent incremental
                        // patches even though their text merges into the button.
                        mark_descendants(nodes, id, seen, depth);
                        return;
                    }
                }
            }
        }
        for child in node.children() {
            self.walk(nodes, *child, transform, !enabled, seen, out, depth + 1);
        }
    }

    fn action_request(
        &self,
        virtual_id: i32,
        android_action: i32,
        value: Option<String>,
    ) -> Option<ActionRequest> {
        let node_id = self
            .items
            .borrow()
            .iter()
            .find(|item| item.id == virtual_id && item.enabled)
            .map(|item| item.node)?;
        let nodes = self.nodes.borrow();
        let node = nodes.get(&node_id)?;
        let action = match android_action {
            16 => Action::Click,
            1 | 64 => {
                if node.supports_action(Action::Focus) {
                    Action::Focus
                } else {
                    Action::ScrollIntoView
                }
            }
            2 => Action::Blur,
            4096 => {
                if node.supports_action(Action::ScrollDown) {
                    Action::ScrollDown
                } else {
                    Action::ScrollRight
                }
            }
            8192 => {
                if node.supports_action(Action::ScrollUp) {
                    Action::ScrollUp
                } else {
                    Action::ScrollLeft
                }
            }
            2097152 if !node.is_read_only() => Action::SetValue,
            _ => return None,
        };
        if !node.supports_action(action) {
            return None;
        }
        let data = if action == Action::SetValue {
            Some(ActionData::Value(value?.into_boxed_str()))
        } else {
            None
        };
        Some(ActionRequest {
            action,
            target_tree: self.tree.get(),
            target_node: node_id,
            data,
        })
    }

    fn snapshot(&self) -> String {
        let focus = self.focus.get();
        let items = self.items.borrow();
        let focused = items
            .iter()
            .find(|item| Some(item.node) == focus)
            .map(|item| item.id)
            .unwrap_or(-1);
        let nodes: Vec<_> = items.iter().map(|item| serde_json::json!({
            "id": item.id, "label": item.label, "value": item.value, "class": item.class_name,
            "bounds": item.bounds, "enabled": item.enabled, "selected": item.selected,
            "checked": item.checked, "password": item.password, "heading": item.heading,
            "editable": item.editable, "actions": item.actions
        })).collect();
        serde_json::json!({ "focus": focused, "nodes": nodes }).to_string()
    }

    fn publish(&self) {
        let snapshot = if self.active.get() {
            self.snapshot()
        } else {
            "{\"focus\":-1,\"nodes\":[]}".into()
        };
        if *self.last_snapshot.borrow() == snapshot {
            return;
        }
        *self.last_snapshot.borrow_mut() = snapshot.clone();
        #[cfg(target_os = "android")]
        java_call(self.token, Some(snapshot), false);
    }
}

impl Drop for A11yBridge {
    fn drop(&mut self) {
        EVENTS
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|(token, _)| *token != self.token);
        #[cfg(target_os = "android")]
        java_call(self.token, None, true);
    }
}

fn mark_descendants(
    nodes: &HashMap<NodeId, Node>,
    id: NodeId,
    seen: &mut HashSet<NodeId>,
    depth: usize,
) {
    if depth > 128 || seen.len() >= MAX_NODES {
        return;
    }
    if let Some(node) = nodes.get(&id) {
        for child in node.children() {
            if seen.insert(*child) {
                mark_descendants(nodes, *child, seen, depth + 1);
            }
        }
    }
}

fn subtree_text(
    nodes: &HashMap<NodeId, Node>,
    id: NodeId,
    seen: &mut HashSet<NodeId>,
    parts: &mut Vec<String>,
    depth: usize,
) {
    if depth > 128 || !seen.insert(id) {
        return;
    }
    if let Some(node) = nodes.get(&id) {
        if node.is_hidden() || node.role() == Role::PasswordInput {
            return;
        }
        if let Some(label) = node
            .label()
            .or_else(|| node.value())
            .filter(|label| !label.trim().is_empty())
        {
            parts.push(label.trim().to_owned());
            return;
        }
        for child in node.children() {
            subtree_text(nodes, *child, seen, parts, depth + 1);
        }
    }
}

fn is_text_input(role: Role) -> bool {
    matches!(
        role,
        Role::TextInput
            | Role::MultilineTextInput
            | Role::SearchInput
            | Role::PasswordInput
            | Role::EmailInput
            | Role::NumberInput
            | Role::PhoneNumberInput
            | Role::UrlInput
    )
}
fn class_name(role: Role) -> &'static str {
    match role {
        Role::Button | Role::DefaultButton | Role::MenuItem => "android.widget.Button",
        Role::CheckBox => "android.widget.CheckBox",
        Role::RadioButton => "android.widget.RadioButton",
        Role::Switch => "android.widget.Switch",
        Role::Image => "android.widget.ImageView",
        role if is_text_input(role) => "android.widget.EditText",
        _ => "android.view.View",
    }
}
fn action_mask(node: &Node) -> u32 {
    let mut mask = 0;
    if node.supports_action(Action::Click) {
        mask |= 1;
    }
    if node.supports_action(Action::Focus) {
        mask |= 2;
    }
    if node.supports_action(Action::ScrollDown) || node.supports_action(Action::ScrollRight) {
        mask |= 4;
    }
    if node.supports_action(Action::ScrollUp) || node.supports_action(Action::ScrollLeft) {
        mask |= 8;
    }
    if node.supports_action(Action::SetValue) && !node.is_read_only() {
        mask |= 16;
    }
    if node.supports_action(Action::Blur) {
        mask |= 32;
    }
    mask
}

#[cfg(target_os = "android")]
fn java_call(token: u64, snapshot: Option<String>, dispose: bool) {
    use jni::{
        JavaVM,
        objects::{JClass, JObject, JValue},
    };
    let Some(app) = crate::platform::android_app() else {
        return;
    };
    let Ok(vm) = (unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }) else {
        return;
    };
    let Ok(mut env) = vm.attach_current_thread() else {
        return;
    };
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
    let result: jni::errors::Result<()> = env.with_local_frame(16, |env| {
        // NativeActivity's native thread has the boot loader as its default.
        // Explicit Activity loader resolves our app-owned service adapter.
        let loader = env
            .call_method(
                &activity,
                "getClassLoader",
                "()Ljava/lang/ClassLoader;",
                &[],
            )?
            .l()?;
        let name = env.new_string("org.jcode.mobile.GpuiAccessibility")?;
        let class = JClass::from(
            env.call_method(
                loader,
                "loadClass",
                "(Ljava/lang/String;)Ljava/lang/Class;",
                &[JValue::Object(name.as_ref())],
            )?
            .l()?,
        );
        if let Some(snapshot) = snapshot {
            let json = env.new_string(snapshot)?;
            env.call_static_method(
                class,
                "update",
                "(Landroid/app/Activity;JLjava/lang/String;)V",
                &[
                    JValue::Object(&activity),
                    JValue::Long(token as i64),
                    JValue::Object(json.as_ref()),
                ],
            )?;
        } else {
            env.call_static_method(
                class,
                if dispose { "dispose" } else { "install" },
                "(Landroid/app/Activity;J)V",
                &[JValue::Object(&activity), JValue::Long(token as i64)],
            )?;
        }
        Ok(())
    });
    if env.exception_check().unwrap_or(true) {
        let _ = env.exception_clear();
    }
    if result.is_err() {
        log::warn!("Android accessibility adapter JNI operation failed");
    }
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_jcode_mobile_GpuiAccessibility_nativeStatus(
    _: jni::JNIEnv<'_>,
    _: jni::objects::JClass<'_>,
    token: jni::sys::jlong,
    enabled: jni::sys::jboolean,
) {
    let _ = enqueue(token as u64, Event::Status(enabled != 0));
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_jcode_mobile_GpuiAccessibility_nativeAction(
    mut env: jni::JNIEnv<'_>,
    _: jni::objects::JClass<'_>,
    token: jni::sys::jlong,
    id: jni::sys::jint,
    action: jni::sys::jint,
    text: jni::objects::JString<'_>,
) -> jni::sys::jboolean {
    let value = if text.is_null() {
        None
    } else {
        match env.get_string(&text) {
            Ok(value) => Some(value.into()),
            Err(_) => {
                let _ = env.exception_clear();
                return 0;
            }
        }
    };
    enqueue(
        token as u64,
        Event::Action {
            virtual_id: id,
            action,
            value,
        },
    ) as jni::sys::jboolean
}

#[cfg(test)]
#[path = "accessibility_tests.rs"]
mod tests;
