//! Host-runnable tests for the AccessKit adapter. These do not establish TalkBack acceptance.
use super::*;
use accesskit::{Rect, Tree};
use std::sync::{Arc, Mutex as TestMutex};

fn node(role: Role, label: &str) -> Node {
    let mut node = Node::new(role);
    node.set_label(label);
    node.set_bounds(Rect {
        x0: 1.0,
        y0: 2.0,
        x1: 101.0,
        y1: 42.0,
    });
    node
}
fn update(nodes: Vec<(NodeId, Node)>, root: Option<u64>, focus: u64) -> TreeUpdate {
    TreeUpdate {
        nodes,
        tree: root.map(|id| Tree::new(NodeId(id))),
        tree_id: TreeId::ROOT,
        focus: NodeId(focus),
    }
}
fn tree(child: Node) -> TreeUpdate {
    let mut root = Node::new(Role::Window);
    root.set_children(vec![NodeId(2)]);
    update(vec![(NodeId(1), root), (NodeId(2), child)], Some(1), 2)
}

#[test]
fn semantic_button_and_json_escape() {
    let bridge = A11yBridge::new();
    let mut button = node(Role::Button, "Send \"message\"\nnow");
    button.add_action(Action::Click);
    bridge.apply(tree(button));
    let snapshot: serde_json::Value = serde_json::from_str(&bridge.snapshot()).unwrap();
    assert_eq!(snapshot["nodes"][0]["label"], "Send \"message\"\nnow");
    assert_eq!(snapshot["nodes"][0]["class"], "android.widget.Button");
    let id = bridge.items.borrow()[0].id;
    let action = bridge.action_request(id, 16, None).unwrap();
    assert_eq!(action.action, Action::Click);
    assert_eq!(action.target_node, NodeId(2));
    assert!(bridge.action_request(id, 4096, None).is_none());
}

#[test]
fn incremental_update_keeps_unchanged_nodes_and_ids() {
    let bridge = A11yBridge::new();
    bridge.apply(tree(node(Role::Label, "before")));
    let id = bridge.items.borrow()[0].id;
    bridge.apply(update(
        vec![(NodeId(2), node(Role::Label, "after"))],
        None,
        2,
    ));
    assert_eq!(bridge.items.borrow()[0].id, id);
    assert_eq!(bridge.items.borrow()[0].label, "after");
    assert_eq!(bridge.nodes.borrow().len(), 2);
}

#[test]
fn repeated_tree_metadata_is_not_a_reset() {
    let bridge = A11yBridge::new();
    bridge.apply(tree(node(Role::Label, "unchanged")));
    let id = bridge.items.borrow()[0].id;
    bridge.apply(update(vec![], Some(1), 2));
    assert_eq!(bridge.items.borrow()[0].id, id);
    assert_eq!(bridge.items.borrow()[0].label, "unchanged");
}

#[test]
fn removed_node_cannot_receive_stale_action() {
    let bridge = A11yBridge::new();
    let mut button = node(Role::Button, "click");
    button.add_action(Action::Click);
    bridge.apply(tree(button));
    let id = bridge.items.borrow()[0].id;
    bridge.apply(update(vec![(NodeId(1), Node::new(Role::Window))], None, 1));
    assert!(bridge.items.borrow().is_empty());
    assert!(bridge.action_request(id, 16, None).is_none());
    assert!(!bridge.nodes.borrow().contains_key(&NodeId(2)));
}

#[test]
fn hidden_subtree_retained_for_unhide_incremental_update() {
    let bridge = A11yBridge::new();
    let mut container = Node::new(Role::GenericContainer);
    container.set_children(vec![NodeId(3)]);
    container.set_hidden();
    let mut initial = tree(container.clone());
    initial.nodes.push((NodeId(3), node(Role::Label, "child")));
    bridge.apply(initial);
    assert!(bridge.items.borrow().is_empty());
    assert!(bridge.nodes.borrow().contains_key(&NodeId(3)));
    container.clear_hidden();
    bridge.apply(update(vec![(NodeId(2), container)], None, 1));
    assert_eq!(bridge.items.borrow()[0].label, "child");
}

#[test]
fn buttons_merge_descendant_text_without_duplicate_virtual_nodes() {
    let bridge = A11yBridge::new();
    let mut button = node(Role::Button, "");
    button.add_action(Action::Click);
    button.set_children(vec![NodeId(3), NodeId(4)]);
    let mut initial = tree(button);
    initial.nodes.extend([
        (NodeId(3), node(Role::Label, "New")),
        (NodeId(4), node(Role::Label, "session")),
    ]);
    bridge.apply(initial);
    assert_eq!(bridge.items.borrow().len(), 1);
    assert_eq!(bridge.items.borrow()[0].label, "New session");
    bridge.apply(update(
        vec![(NodeId(4), node(Role::Label, "chat"))],
        None,
        1,
    ));
    assert_eq!(bridge.items.borrow()[0].label, "New chat");
}

#[test]
fn disabled_ancestor_blocks_actions() {
    let bridge = A11yBridge::new();
    let mut button = node(Role::Button, "send");
    button.add_action(Action::Click);
    let mut initial = tree(button);
    initial.nodes[0].1.set_disabled();
    bridge.apply(initial);
    let item = &bridge.items.borrow()[0];
    assert!(!item.enabled);
    assert!(bridge.action_request(item.id, 16, None).is_none());
}

#[test]
fn text_focus_set_value_and_readonly_contract() {
    let bridge = A11yBridge::new();
    let mut input = node(Role::TextInput, "Message");
    input.add_action(Action::SetValue);
    input.add_action(Action::Focus);
    bridge.apply(tree(input.clone()));
    let id = bridge.items.borrow()[0].id;
    assert_eq!(
        bridge.action_request(id, 1, None).unwrap().action,
        Action::Focus
    );
    assert_eq!(
        bridge
            .action_request(id, 2097152, Some("hello".into()))
            .unwrap()
            .data,
        Some(ActionData::Value("hello".into()))
    );
    assert!(bridge.action_request(id, 2097152, None).is_none());
    input.set_read_only();
    bridge.apply(update(vec![(NodeId(2), input)], None, 2));
    assert!(
        bridge
            .action_request(id, 2097152, Some("changed".into()))
            .is_none()
    );
}

#[test]
fn password_values_never_serialized() {
    let bridge = A11yBridge::new();
    let mut input = node(Role::PasswordInput, "Password");
    input.set_value("very-secret-value");
    bridge.apply(tree(input));
    let json = bridge.snapshot();
    assert!(!json.contains("very-secret-value"));
    assert_eq!(bridge.items.borrow()[0].value, "");
    assert!(bridge.items.borrow()[0].password);
}

#[test]
fn nested_transforms_and_physical_pixel_scale() {
    let bridge = A11yBridge::new();
    bridge.set_scale(2.0);
    let mut initial = tree(node(Role::Label, "position"));
    initial.nodes[0]
        .1
        .set_transform(Affine::new([1., 0., 0., 1., 10., 20.]));
    initial.nodes[1]
        .1
        .set_transform(Affine::new([1., 0., 0., 1., 2., 3.]));
    bridge.apply(initial);
    assert_eq!(bridge.items.borrow()[0].bounds, [26., 50., 226., 130.]);
}

#[test]
fn invalid_bounds_and_cycles_do_not_expose_nodes_or_recurse_forever() {
    let bridge = A11yBridge::new();
    let mut child = node(Role::Label, "bad");
    child.set_bounds(Rect {
        x0: f64::NAN,
        y0: 0.,
        x1: 10.,
        y1: 10.,
    });
    child.set_children(vec![NodeId(1)]);
    bridge.apply(tree(child));
    assert!(bridge.items.borrow().is_empty());
}

#[test]
fn root_replacement_does_not_reuse_android_virtual_ids() {
    let bridge = A11yBridge::new();
    bridge.apply(tree(node(Role::Label, "first")));
    let old = bridge.items.borrow()[0].id;
    let mut replacement = tree(node(Role::Label, "second"));
    replacement.tree = Some(Tree::new(NodeId(10)));
    replacement.nodes[0].0 = NodeId(10);
    bridge.apply(replacement);
    assert_ne!(bridge.items.borrow()[0].id, old);
}

#[test]
fn callbacks_only_run_when_gpui_thread_drains_queue() {
    let bridge = A11yBridge::new();
    let actions = Arc::new(TestMutex::new(Vec::new()));
    let captured = actions.clone();
    let mut button = node(Role::Button, "click");
    button.add_action(Action::Click);
    bridge.init(A11yCallbacks {
        activation: Box::new(move || Some(tree(button.clone()))),
        action: Box::new(move |request| {
            captured
                .lock()
                .unwrap()
                .push((std::thread::current().id(), request))
        }),
        deactivation: Box::new(|| {}),
    });
    let token = bridge.token;
    std::thread::spawn(move || assert!(enqueue(token, Event::Status(true))))
        .join()
        .unwrap();
    assert!(bridge.items.borrow().is_empty());
    bridge.process_events();
    let id = bridge.items.borrow()[0].id;
    std::thread::spawn(move || {
        assert!(enqueue(
            token,
            Event::Action {
                virtual_id: id,
                action: 16,
                value: None
            }
        ))
    })
    .join()
    .unwrap();
    assert!(actions.lock().unwrap().is_empty());
    bridge.process_events();
    let actions = actions.lock().unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].0, std::thread::current().id());
    assert_eq!(actions[0].1.action, Action::Click);
}

#[test]
fn deactivation_clears_tree_and_rejects_actions() {
    let bridge = A11yBridge::new();
    let deactivated = Arc::new(TestMutex::new(false));
    let captured = deactivated.clone();
    bridge.init(A11yCallbacks {
        activation: Box::new(|| Some(tree(node(Role::Label, "label")))),
        action: Box::new(|_| panic!("inactive action must not dispatch")),
        deactivation: Box::new(move || *captured.lock().unwrap() = true),
    });
    enqueue(bridge.token, Event::Status(true));
    bridge.process_events();
    enqueue(bridge.token, Event::Status(false));
    bridge.process_events();
    assert!(*deactivated.lock().unwrap());
    assert!(bridge.items.borrow().is_empty());
    enqueue(
        bridge.token,
        Event::Action {
            virtual_id: 1,
            action: 16,
            value: None,
        },
    );
    bridge.process_events();
}

#[test]
fn embedded_tree_cannot_overwrite_root_node_id_space() {
    let bridge = A11yBridge::new();
    bridge.apply(tree(node(Role::Label, "root label")));
    let mut embedded = tree(node(Role::Label, "wrong subtree"));
    embedded.tree_id = TreeId("b1e3fbcb-b6aa-4a3d-8b42-141e0ee5dca4".parse().unwrap());
    bridge.apply(embedded);
    assert_eq!(bridge.items.borrow()[0].label, "root label");
    assert_eq!(bridge.tree.get(), TreeId::ROOT);
}

#[test]
fn unknown_bridge_events_cannot_trigger_another_window() {
    let bridge = A11yBridge::new();
    let other = A11yBridge::new();
    enqueue(other.token, Event::Status(true));
    bridge.process_events();
    assert!(!bridge.active.get());
}
