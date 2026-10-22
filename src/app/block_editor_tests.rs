use super::{Message, ModalKind, OpenCADStudio};
use codec::entities::{Insert, Line};
use codec::types::Vector3;
use codec::{EntityType, Handle};

fn fixture() -> (OpenCADStudio, Handle) {
    let mut app = OpenCADStudio::new_for_test();
    app.automation_op(r#"{"op":"new"}"#);
    let i = app.active_tab;
    app.tabs[i].scene.define_block_raw(
        "EDIT_ME",
        Vector3::ZERO,
        vec![EntityType::Line(Line::from_points(
            Vector3::ZERO,
            Vector3::UNIT_X,
        ))],
    );
    let reference = app.tabs[i]
        .scene
        .document
        .add_entity(EntityType::Insert(Insert::new("EDIT_ME", Vector3::ZERO)))
        .unwrap();
    app.tabs[i].dirty = false;
    (app, reference)
}

fn begin(app: &mut OpenCADStudio, reference: Handle) {
    let _ = app.dispatch_command(&format!("BEDIT_BEGIN:{}", reference.value()));
    assert!(app.tabs[app.active_tab].active_block_edit.is_some());
}

fn circle(app: &mut OpenCADStudio, x: f64) {
    let result = app.automation_op(&format!(r#"{{"op":"run","cmd":"CIRCLE {x},0 2"}}"#));
    assert_eq!(result["ok"], true, "{result}");
    app.finish_pending_history(app.active_tab);
}

fn members(app: &OpenCADStudio) -> Vec<Handle> {
    app.tabs[app.active_tab]
        .scene
        .document
        .block_records
        .get("EDIT_ME")
        .unwrap()
        .entity_handles
        .clone()
}

fn main_depth(app: &OpenCADStudio) -> usize {
    let tab = &app.tabs[app.active_tab];
    tab.block_edit_history
        .as_ref()
        .map(|h| h.main.undo_stack.len())
        .unwrap_or(tab.history.undo_stack.len())
}

fn feed(app: &mut OpenCADStudio, text: &str) {
    let _ = app.update(Message::CommandInput(text.to_string()));
    let _ = app.update(Message::CommandSubmit);
}

fn authored_points(app: &OpenCADStudio) -> Vec<codec::objects::BlockPointParameter> {
    let tab = &app.tabs[app.active_tab];
    let definition = tab
        .scene
        .document
        .block_records
        .get("EDIT_ME")
        .unwrap()
        .handle;
    crate::scene::block_point_parameter::parameters(&tab.scene.document, definition)
        .into_iter()
        .map(|(_, parameter)| parameter.clone())
        .collect()
}

fn distance_parameter_count(app: &OpenCADStudio) -> usize {
    let tab = &app.tabs[app.active_tab];
    let definition = tab
        .scene
        .document
        .block_records
        .get("EDIT_ME")
        .unwrap()
        .handle;
    crate::scene::block_distance_parameter::parameters(&tab.scene.document, definition).len()
}

fn orientation_parameter_count(app: &OpenCADStudio) -> usize {
    let tab = &app.tabs[app.active_tab];
    let definition = tab
        .scene
        .document
        .block_records
        .get("EDIT_ME")
        .unwrap()
        .handle;
    crate::scene::block_orientation_parameter::parameters(&tab.scene.document, definition).len()
}

#[test]
fn orientation_parameter_batch_numeric_rotation_local_history_save_and_parent_undo() {
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    let i = app.active_tab;
    for (index, (kind, inputs)) in [
        ("ROTATION", vec!["0,0", "2", "90"]),
        ("ALIGNMENT", vec!["0,0", "0,2"]),
        ("FLIP", vec!["0,0", "0,2", "3,3"]),
    ]
    .into_iter()
    .enumerate()
    {
        let before = app.tabs[i].scene.document.clone();
        let _ = app.dispatch_command(&format!("BPARAMETER {kind}"));
        for text in &inputs[..inputs.len() - 1] {
            feed(&mut app, text);
            assert_eq!(app.tabs[i].scene.document, before, "incomplete placement");
        }
        feed(&mut app, inputs.last().unwrap());
        assert_eq!(orientation_parameter_count(&app), index + 1, "{kind}");
        assert_eq!(app.tabs[i].history.undo_stack.len(), index + 1);
        assert!(app.tabs[i].scene.preview_wires.is_empty());
        app.undo_steps(1);
        assert_eq!(orientation_parameter_count(&app), index);
        app.redo_steps(1);
        assert_eq!(orientation_parameter_count(&app), index + 1);
    }
    let definition = app.tabs[i].active_block_edit_session().unwrap().br_handle;
    let p = &crate::scene::block_orientation_parameter::parameters(
        &app.tabs[i].scene.document,
        definition,
    )[0];
    assert!((p.native.definition_end_point - Vector3::new(0.0, 2.0, 0.0)).length() < 1e-10);
    assert_eq!(p.reference, Vector3::new(2.0, 0.0, 0.0));
    let editor = app.tabs[i].scene.document.clone();
    let _ = app.dispatch_command("BTESTBLOCK");
    assert!(app.tabs[app.active_tab]
        .active_block_edit_session()
        .is_none());
    assert_eq!(
        app.tabs[app.active_tab]
            .scene
            .document
            .entities_in_block("EDIT_ME")
            .count(),
        1
    );
    let _ = app.dispatch_command("BCLOSETESTBLOCK");
    assert_eq!(app.tabs[i].scene.document, editor);
    let _ = app.run_command_line("UCS ORIGIN 1,2,0");
    for p in crate::scene::block_orientation_parameter::parameters(
        &app.tabs[i].scene.document,
        definition,
    ) {
        assert_eq!(
            p.native.definition_base_point,
            Vector3::new(-1.0, -2.0, 0.0)
        );
    }
    let _ = app.dispatch_command("BSAVE");
    let _ = app.dispatch_command("BEDIT_DISCARD");
    assert_eq!(orientation_parameter_count(&app), 3);
    app.undo_steps(1);
    assert_eq!(orientation_parameter_count(&app), 0);
    app.redo_steps(1);
    assert_eq!(orientation_parameter_count(&app), 3);
}

#[test]
fn orientation_parameter_chooser_cancels_invalid_picks_and_rotation_default_angle() {
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    let i = app.active_tab;
    let before = app.tabs[i].scene.document.clone();
    for (kind, maximum) in [("R", 2), ("A", 1), ("F", 2)] {
        for picks in 0..=maximum {
            let _ = app.dispatch_command("BPARAMETER");
            feed(&mut app, kind);
            if picks > 0 {
                feed(&mut app, "0,0");
                feed(&mut app, "0,0");
                assert_eq!(app.tabs[i].scene.document, before);
            }
            if picks > 1 {
                feed(&mut app, "0,2");
            }
            let _ = app.update(Message::CommandEscape);
            assert_eq!(app.tabs[i].scene.document, before);
            assert!(app.tabs[i].history.undo_stack.is_empty());
        }
    }
    let _ = app.dispatch_command("BPARAMETER ROTATION");
    feed(&mut app, "0,0");
    feed(&mut app, "-2");
    assert_eq!(app.tabs[i].scene.document, before);
    feed(&mut app, "2");
    feed(&mut app, "");
    assert_eq!(orientation_parameter_count(&app), 1);
    let _ = app.dispatch_command("BEDIT_DISCARD");
    assert_eq!(orientation_parameter_count(&app), 0);
    let _ = app.dispatch_command("LINE");
    for kind in ["ROTATION", "ALIGNMENT", "FLIP"] {
        let _ = app.dispatch_command(&format!("BPARAMETER {kind}"));
        assert_eq!(app.tabs[i].active_cmd.as_ref().unwrap().name(), "LINE");
    }
}

#[test]
fn orientation_parameter_stale_context_and_invalid_result_do_not_start_history() {
    use crate::scene::block_orientation_parameter::Kind;
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    let i = app.active_tab;
    let definition = app.tabs[i].active_block_edit_session().unwrap().br_handle;
    let id = app.tabs[i].id;
    let before = app.tabs[i].scene.document.clone();
    for (document_id, end) in [
        (id + 1, glam::DVec3::Y),
        (id, glam::DVec3::ZERO),
        (id, glam::DVec3::splat(f64::NAN)),
    ] {
        let _ = app.handle_place_block_orientation_parameter(
            crate::command::CmdResult::PlaceBlockOrientationParameter {
                document_id,
                definition,
                kind: Kind::Flip,
                base: glam::DVec3::ZERO,
                end,
                reference: glam::DVec3::X,
            },
        );
        assert_eq!(app.tabs[i].scene.document, before);
        assert!(app.tabs[i].history.undo_stack.is_empty());
    }
}

#[test]
fn distance_parameter_batch_three_picks_local_undo_redo_and_saved_parent_undo() {
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    let i = app.active_tab;
    for (index, kind) in ["LINEAR", "POLAR", "XY"].iter().enumerate() {
        let before = app.tabs[i].scene.document.clone();
        let _ = app.dispatch_command(&format!("BPARAMETER {kind}"));
        feed(&mut app, "2,3");
        feed(&mut app, "6,7");
        assert_eq!(
            app.tabs[i].scene.document, before,
            "base and endpoint are preview only"
        );
        feed(&mut app, "1,8");
        assert_eq!(distance_parameter_count(&app), index + 1);
        assert_eq!(app.tabs[i].history.undo_stack.len(), index + 1);
        assert!(app.tabs[i].scene.preview_wires.is_empty());
        app.undo_steps(1);
        assert_eq!(distance_parameter_count(&app), index);
        app.redo_steps(1);
        assert_eq!(distance_parameter_count(&app), index + 1);
    }
    let _ = app.run_command_line("BPARAMETER POINT 0,0 1,1");
    let _ = app.run_command_line("BPARAMETER VISIBILITY 0,0");
    let editor = app.tabs[i].scene.document.clone();
    let _ = app.dispatch_command("BTESTBLOCK");
    assert!(app.tabs[app.active_tab]
        .active_block_edit_session()
        .is_none());
    assert_eq!(
        app.tabs[app.active_tab]
            .scene
            .document
            .entities_in_block("EDIT_ME")
            .count(),
        1,
        "parameters and their editor graphics add no drawing geometry"
    );
    assert_eq!(distance_parameter_count(&app), 3);
    let _ = app.dispatch_command("BCLOSETESTBLOCK");
    assert_eq!(app.active_tab, i);
    assert_eq!(app.tabs[i].scene.document, editor);
    let _ = app.run_command_line("UCS ORIGIN 1,2,0");
    let definition = app.tabs[i].active_block_edit_session().unwrap().br_handle;
    for p in
        crate::scene::block_distance_parameter::parameters(&app.tabs[i].scene.document, definition)
    {
        assert_eq!(p.native.definition_base_point, Vector3::new(1.0, 1.0, 0.0));
        assert_eq!(p.native.definition_end_point, Vector3::new(5.0, 5.0, 0.0));
    }
    let _ = app.dispatch_command("BSAVE");
    let _ = app.dispatch_command("BEDIT_DISCARD");
    assert_eq!(distance_parameter_count(&app), 3);
    app.undo_steps(1);
    assert_eq!(distance_parameter_count(&app), 0);
    app.redo_steps(1);
    assert_eq!(distance_parameter_count(&app), 3);
}

#[test]
fn distance_parameter_cancel_chooser_invalid_endpoint_and_discard_are_safe() {
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    let i = app.active_tab;
    let before = app.tabs[i].scene.document.clone();
    for kind in ["Linear", "Polar", "XY"] {
        for picks in 0..3 {
            let _ = app.dispatch_command("BPARAMETER");
            feed(&mut app, kind);
            if picks > 0 {
                feed(&mut app, "2,3");
            }
            if picks > 1 {
                feed(&mut app, "6,7");
            }
            let _ = app.update(Message::CommandEscape);
            assert_eq!(app.tabs[i].scene.document, before);
            assert!(app.tabs[i].history.undo_stack.is_empty());
        }
        let _ = app.dispatch_command(&format!("BPARAMETER {kind}"));
        feed(&mut app, "2,3");
        feed(&mut app, "2,3");
        assert_eq!(app.tabs[i].scene.document, before);
        assert!(
            app.tabs[i].active_cmd.is_some(),
            "invalid endpoint can be retried"
        );
        feed(&mut app, "6,7");
        feed(&mut app, "1,8");
        assert_eq!(distance_parameter_count(&app), 1);
        app.undo_steps(1);
    }
    let _ = app.run_command_line("BPARAMETER LINEAR 0,0 1,1 2,2");
    let _ = app.dispatch_command("BEDIT_DISCARD");
    assert_eq!(distance_parameter_count(&app), 0);
    let _ = app.dispatch_command("LINE");
    for kind in ["LINEAR", "POLAR", "XY"] {
        let _ = app.dispatch_command(&format!("BPARAMETER {kind}"));
        assert_eq!(app.tabs[i].active_cmd.as_ref().unwrap().name(), "LINE");
    }
}

#[test]
fn distance_parameter_stale_context_and_invalid_result_do_not_start_history() {
    use crate::scene::block_distance_parameter::Kind;
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    let i = app.active_tab;
    let definition = app.tabs[i].active_block_edit_session().unwrap().br_handle;
    let id = app.tabs[i].id;
    let before = app.tabs[i].scene.document.clone();
    for (document_id, end) in [
        (id + 1, glam::DVec3::ONE),
        (id, glam::DVec3::ZERO),
        (id, glam::DVec3::splat(f64::NAN)),
    ] {
        let _ = app.handle_place_block_distance_parameter(
            crate::command::CmdResult::PlaceBlockDistanceParameter {
                document_id,
                definition,
                kind: Kind::Linear,
                base: glam::DVec3::ZERO,
                end,
                label: glam::DVec3::ZERO,
            },
        );
        assert_eq!(app.tabs[i].scene.document, before);
        assert!(app.tabs[i].history.undo_stack.is_empty());
    }
}

#[test]
fn point_parameter_placement_two_picks_one_undo_and_repeated_names() {
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    let i = app.active_tab;
    let before = app.tabs[i].scene.document.clone();
    let _ = app.dispatch_command("BPARAMETER POINT");
    feed(&mut app, "3,4,0");
    assert_eq!(
        app.tabs[i].scene.document, before,
        "first pick is preview only"
    );
    feed(&mut app, "6,7,0");
    assert_eq!(authored_points(&app)[0].position_name, "Position1");
    assert_eq!(
        authored_points(&app)[0].definition_label_point,
        Vector3::new(6.0, 7.0, 0.0)
    );
    assert_eq!(app.tabs[i].history.undo_stack.len(), 1);
    assert!(app.block_edit_dirty(i));
    app.undo_steps(1);
    assert!(authored_points(&app).is_empty());
    app.redo_steps(1);
    assert_eq!(authored_points(&app).len(), 1);
    let _ = app.run_command_line("BPARAMETER POINT 8,9 10,11");
    assert_eq!(authored_points(&app)[1].position_name, "Position2");
    assert_eq!(app.tabs[i].history.undo_stack.len(), 2);
}

#[test]
fn point_parameter_placement_cancel_at_both_prompts_is_inert() {
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    let i = app.active_tab;
    let before = app.tabs[i].scene.document.clone();
    for picked in [false, true] {
        let _ = app.dispatch_command("BPARAMETER POINT");
        if picked {
            feed(&mut app, "3,4");
        }
        let _ = app.update(Message::CommandEscape);
        assert_eq!(app.tabs[i].scene.document, before);
        assert!(app.tabs[i].history.undo_stack.is_empty());
        assert!(!app.block_edit_dirty(i));
    }
    let _ = app.dispatch_command("BPARAMETER POINT");
    feed(&mut app, "3,4");
    feed(&mut app, "");
    assert_eq!(app.tabs[i].scene.document, before);
    assert!(app.tabs[i].history.undo_stack.is_empty());
}

#[test]
fn point_parameter_placement_chooser_works_with_existing_visibility_and_reframes() {
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    let i = app.active_tab;
    let _ = app.run_command_line("BPARAMETER VISIBILITY 0,0");
    let _ = app.dispatch_command("BPARAMETER");
    feed(&mut app, "Point");
    feed(&mut app, "3,4,5");
    feed(&mut app, "6,7,5");
    assert_eq!(authored_points(&app).len(), 1);
    assert!(app.block_visibility_context().is_some());
    let _ = app.run_command_line("UCS ORIGIN 1,2,3");
    assert_eq!(
        authored_points(&app)[0].parameter.definition_point,
        Vector3::new(2.0, 2.0, 2.0)
    );
    assert_eq!(
        authored_points(&app)[0].definition_label_point,
        Vector3::new(5.0, 5.0, 2.0)
    );
    app.undo_steps(1);
    assert_eq!(
        authored_points(&app)[0].parameter.definition_point,
        Vector3::new(3.0, 4.0, 5.0)
    );
    let _ = app.dispatch_command("BSAVE");
    assert!(!app.block_edit_dirty(i));
    let _ = app.dispatch_command("BEDIT_DISCARD");
    assert_eq!(authored_points(&app).len(), 1);
    app.undo_steps(1);
    assert!(
        authored_points(&app).is_empty(),
        "saved block undo restores the parent drawing"
    );
}

#[test]
fn point_parameter_placement_discard_and_stale_context_leave_no_objects() {
    let (mut app, reference) = fixture();
    let i = app.active_tab;
    let before = app.tabs[i].scene.document.clone();
    begin(&mut app, reference);
    let definition = app.tabs[i].active_block_edit_session().unwrap().br_handle;
    let start = app.tabs[i].scene.document.clone();
    let id = app.tabs[i].id;
    for (document, location) in [
        (id + 1, glam::DVec3::ZERO),
        (id, glam::DVec3::splat(f64::NAN)),
    ] {
        let _ = app.handle_place_block_point_parameter(
            crate::command::CmdResult::PlaceBlockPointParameter {
                document_id: document,
                definition,
                location,
                label: glam::DVec3::ZERO,
            },
        );
        assert_eq!(app.tabs[i].scene.document, start);
    }
    let _ = app.run_command_line("BPARAMETER POINT 3,4 6,7");
    let _ = app.dispatch_command("BEDIT_DISCARD");
    let mut after = app.tabs[i].scene.document.clone();
    after.vports = before.vports.clone();
    assert_eq!(after, before);
    let closed = app.tabs[i].scene.document.clone();
    let _ = app.dispatch_command("LINE");
    let _ = app.dispatch_command("BPARAMETER POINT");
    assert_eq!(app.tabs[i].active_cmd.as_ref().unwrap().name(), "LINE");
    assert_eq!(app.tabs[i].scene.document, closed);
}

fn visibility_editor() -> (OpenCADStudio, crate::ui::window::block_visibility::Target) {
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    let _ = app.run_command_line("BPARAMETER VISIBILITY 0,0");
    let target = app
        .block_visibility_context()
        .expect("placed parameter")
        .target;
    (app, target)
}

fn visibility_action(app: &mut OpenCADStudio, action: crate::ui::window::block_visibility::Action) {
    let _ = app.update(Message::BlockVisibility(action));
}

#[test]
fn visibility_ribbon_manager_cancel_validation_and_single_undo() {
    use crate::ui::window::block_visibility::Action as A;
    let (mut app, target) = visibility_editor();
    let i = app.active_tab;
    let before = app.tabs[i].scene.document.clone();
    let depth = app.tabs[i].history.undo_stack.len();
    visibility_action(&mut app, A::Open(target));
    visibility_action(&mut app, A::Name("Alternate".into()));
    visibility_action(&mut app, A::New);
    assert_eq!(app.tabs[i].scene.document, before, "dialog is staged");
    let _ = app.update(Message::CommandEscape);
    assert!(app.block_visibility.is_none());
    assert_eq!(app.tabs[i].history.undo_stack.len(), depth);
    assert_eq!(app.tabs[i].scene.document, before);
    visibility_action(&mut app, A::Open(target));
    visibility_action(&mut app, A::Name("visibilitystate0".into()));
    visibility_action(&mut app, A::New);
    assert!(app.block_visibility.as_ref().unwrap().error.is_some());
    visibility_action(&mut app, A::Delete);
    assert_eq!(
        app.block_visibility.as_ref().unwrap().edited.states.len(),
        1
    );
    visibility_action(&mut app, A::Name("Alternate".into()));
    visibility_action(&mut app, A::New);
    visibility_action(&mut app, A::SetCurrent);
    visibility_action(&mut app, A::Name("Alternate renamed".into()));
    visibility_action(&mut app, A::Rename);
    visibility_action(&mut app, A::Move(true));
    visibility_action(&mut app, A::Apply);
    assert_eq!(app.tabs[i].history.undo_stack.len(), depth + 1);
    assert_eq!(
        app.block_visibility_context().unwrap().current.as_deref(),
        Some("Alternate renamed")
    );
    app.undo_steps(1);
    assert_eq!(
        app.block_visibility_context().unwrap().names,
        ["VisibilityState0"]
    );
    app.redo_steps(1);
    assert_eq!(
        app.block_visibility_context().unwrap().names,
        ["Alternate renamed", "VisibilityState0"]
    );
    assert_eq!(
        app.block_visibility_context().unwrap().current.as_deref(),
        Some("Alternate renamed")
    );
}

#[test]
fn visibility_ribbon_new_modes_state_selection_and_native_roundtrip() {
    use crate::ui::window::block_visibility::{Action as A, NewVisibility as V};
    let (mut app, target) = visibility_editor();
    let i = app.active_tab;
    let member = members(&app)[0];
    visibility_action(&mut app, A::Open(target));
    for (name, mode) in [("Hidden", V::Hide), ("Shown", V::Show), ("Copy", V::Keep)] {
        visibility_action(&mut app, A::Name(name.into()));
        visibility_action(&mut app, A::NewVisibility(mode));
        visibility_action(&mut app, A::New);
    }
    visibility_action(&mut app, A::Apply);
    let parameter =
        crate::scene::block_visibility::parameter(&app.tabs[i].scene.document, target.definition)
            .unwrap();
    assert!(parameter.states[0].visible_blocks.contains(&member));
    assert!(parameter.states[1].visible_blocks.is_empty());
    assert!(parameter.states[2].visible_blocks.contains(&member));
    assert!(parameter.states[3].visible_blocks.contains(&member));
    visibility_action(&mut app, A::Current(target, "Hidden".into()));
    assert!(
        app.tabs[i]
            .scene
            .document
            .get_entity(member)
            .unwrap()
            .common()
            .invisible
    );
    visibility_action(&mut app, A::Current(target, "Shown".into()));
    visibility_action(&mut app, A::Current(target, "Copy".into()));
    app.undo_steps(1);
    assert_eq!(
        app.block_visibility_context().unwrap().current.as_deref(),
        Some("Shown"),
        "identical states retain their identity in history"
    );
    app.redo_steps(1);
    assert_eq!(
        app.block_visibility_context().unwrap().current.as_deref(),
        Some("Copy")
    );
    app.save_block_edit(i);
    for ext in ["dwg", "dxf"] {
        let document = &app.tabs[i].scene.document;
        let bytes = crate::io::save_to_bytes(document, ext, document.version).unwrap();
        let loaded = crate::io::load_bytes(&format!("visibility.{ext}"), bytes).unwrap();
        let parameter =
            crate::scene::block_visibility::parameter(&loaded, target.definition).unwrap();
        assert_eq!(
            parameter
                .states
                .iter()
                .map(|state| state.name.as_str())
                .collect::<Vec<_>>(),
            ["VisibilityState0", "Hidden", "Shown", "Copy"]
        );
        assert!(parameter.states[1].visible_blocks.is_empty());
        assert!(parameter.states[2].visible_blocks.contains(&member));
    }
}

#[test]
fn visibility_ribbon_mode_and_make_visible_cancel_do_not_change_saved_flags() {
    use crate::ui::window::block_visibility::Action as A;
    let (mut app, target) = visibility_editor();
    let i = app.active_tab;
    let member = members(&app)[0];
    app.tabs[i].scene.select_entity(member, true);
    let _ = app.run_command_line("BVHIDE");
    app.finish_pending_history(i);
    let before = app.tabs[i].scene.document.clone();
    let depth = app.tabs[i].history.undo_stack.len();
    let dirty = app.tabs[i].dirty;
    let wires = |app: &OpenCADStudio| {
        app.tabs[i]
            .scene
            .entity_wires()
            .iter()
            .filter(|wire| crate::scene::Scene::handle_from_wire_name(&wire.name) == Some(member))
            .cloned()
            .collect::<Vec<_>>()
    };
    assert!(wires(&app).is_empty());
    visibility_action(&mut app, A::Mode(target));
    assert!(
        !wires(&app).is_empty(),
        "hidden member is rendered for picking"
    );
    assert_eq!(app.tabs[i].scene.document, before);
    visibility_action(&mut app, A::Mode(target));
    assert!(wires(&app).is_empty());
    let _ = app.run_command_line("BVSHOW");
    app.sync_block_visibility_display();
    assert!(!wires(&app).is_empty());
    let _ = app.update(Message::CommandEscape);
    assert!(wires(&app).is_empty());
    assert_eq!(app.tabs[i].scene.document, before);
    assert_eq!(app.tabs[i].history.undo_stack.len(), depth);
    assert_eq!(app.tabs[i].dirty, dirty);
    let _ = app.run_command_line("BVMODE 1");
    assert!(app.block_visibility_mode);
    let _ = app.run_command_line("SETVAR BVMODE 2");
    assert!(app.block_visibility_mode);
    let _ = app.run_command_line("BVMODE 0");
    assert!(!app.block_visibility_mode);
    assert!(
        app.pending_setvar.is_none(),
        "inline variable must close its probe prompt"
    );
    let _ = app.run_command_line("BVSHOW");
    app.tabs[i].scene.select_entity(member, true);
    let result = app.tabs[i]
        .active_cmd
        .as_mut()
        .unwrap()
        .on_selection_complete(vec![member]);
    let _ = app.apply_cmd_result(result);
    feed(&mut app, "");
    assert!(
        !app.tabs[i]
            .scene
            .document
            .get_entity(member)
            .unwrap()
            .common()
            .invisible
    );
    app.undo_steps(1);
    assert!(
        app.tabs[i]
            .scene
            .document
            .get_entity(member)
            .unwrap()
            .common()
            .invisible,
        "undo uses the hidden flags from before picking"
    );
}

#[test]
fn visibility_ribbon_manager_rejects_stale_target_and_stale_document() {
    use crate::ui::window::block_visibility::Action as A;
    let (mut app, target) = visibility_editor();
    visibility_action(&mut app, A::Open(target));
    visibility_action(&mut app, A::Name("Alternate".into()));
    visibility_action(&mut app, A::New);
    app.tabs[app.active_tab].scene.bump_geometry();
    visibility_action(&mut app, A::Apply);
    assert!(app.block_visibility.as_ref().unwrap().error.is_some());
    assert_eq!(
        app.block_visibility_context().unwrap().names,
        ["VisibilityState0"]
    );
    let _ = app.update(Message::CloseModal);
    let mut stale = target;
    stale.document += 1;
    visibility_action(&mut app, A::Open(stale));
    assert!(app.block_visibility.is_none());
    assert!(app.active_modal.is_none());
    let _ = app.run_command_line("BVSTATE DIALOG");
    assert_eq!(app.active_modal, Some(ModalKind::BlockVisibility));
}

#[test]
fn local_undo_preserves_imported_native_networks_when_constraints_are_unchanged() {
    use codec::objects::{
        AssocNetwork, AssociativeData, AssociativeObject, Dictionary, ObjectType,
    };
    let (mut app, reference) = fixture();
    let i = app.active_tab;
    let doc = &mut app.tabs[i].scene.document;
    let owner = doc.block_records.get("EDIT_ME").unwrap().handle;
    let dictionary_handle = doc.allocate_handle();
    let network_handle = doc.allocate_handle();
    let mut dictionary = Dictionary::new();
    dictionary.handle = dictionary_handle;
    dictionary.owner = owner;
    dictionary.hard_owner = true;
    dictionary.add_entry("ACAD_ASSOCNETWORK", network_handle);
    doc.objects
        .insert(dictionary_handle, ObjectType::Dictionary(dictionary));
    let network = AssociativeObject {
        handle: network_handle,
        owner: dictionary_handle,
        reactors: vec![dictionary_handle],
        dxf_name: "ASSOCNETWORK".into(),
        cpp_class_name: "AcDbAssocNetwork".into(),
        data: AssociativeData::Network(AssocNetwork::default()),
        ..Default::default()
    };
    doc.objects
        .insert(network_handle, ObjectType::Associative(network));
    begin(&mut app, reference);
    let original_objects = app.tabs[i].scene.document.objects.clone();
    circle(&mut app, 1.0);
    assert!(!app.save_block_edit_as(i, "COPY"));
    assert_eq!(app.tabs[i].scene.document.objects, original_objects);
    app.undo_steps(1);
    assert_eq!(members(&app).len(), 1);
    assert_eq!(app.tabs[i].scene.document.objects, original_objects);
    app.redo_steps(1);
    assert_eq!(members(&app).len(), 2);
    assert_eq!(app.tabs[i].scene.document.objects, original_objects);
}

#[test]
fn save_as_forks_unsaved_edits_and_undoes_as_one_drawing_step() {
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    circle(&mut app, 1.0);
    app.save_block_edit(app.active_tab);
    let saved = members(&app);
    circle(&mut app, 2.0);
    let edited = members(&app).len();
    assert!(app.save_block_edit_as(app.active_tab, "  COPY  "));
    let i = app.active_tab;
    assert_eq!(members(&app), saved);
    assert_eq!(
        app.tabs[i].active_block_edit_session().unwrap().block_name,
        "COPY"
    );
    assert_eq!(
        app.tabs[i]
            .scene
            .document
            .block_records
            .get("COPY")
            .unwrap()
            .entity_handles
            .len(),
        edited
    );
    assert_eq!(main_depth(&app), 2);
    assert!(app.tabs[i].history.undo_stack.is_empty());
    assert!(!app.block_edit_dirty(i));
    app.undo_steps(1);
    assert!(app.tabs[i]
        .scene
        .document
        .block_records
        .get("COPY")
        .is_some());
    circle(&mut app, 3.0);
    app.close_block_edit(i, true);
    assert_eq!(
        app.tabs[i]
            .scene
            .document
            .block_records
            .get("COPY")
            .unwrap()
            .entity_handles
            .len(),
        edited
    );
    app.undo_steps(1);
    assert!(app.tabs[i]
        .scene
        .document
        .block_records
        .get("COPY")
        .is_none());
    assert_eq!(members(&app), saved);
    app.redo_steps(1);
    assert_eq!(
        app.tabs[i]
            .scene
            .document
            .block_records
            .get("COPY")
            .unwrap()
            .entity_handles
            .len(),
        edited
    );
}

#[test]
fn save_as_name_refusals_do_not_change_the_session() {
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    circle(&mut app, 1.0);
    let i = app.active_tab;
    let document = app.tabs[i].scene.document.clone();
    let depth = app.tabs[i].history.undo_stack.len();
    for name in ["", "bad/name", "edit_me", "EDIT_ME"] {
        assert!(!app.save_block_edit_as(i, name));
        assert_eq!(app.tabs[i].scene.document, document);
        assert_eq!(app.tabs[i].history.undo_stack.len(), depth);
        assert_eq!(main_depth(&app), 0);
        assert_eq!(
            app.tabs[i].active_block_edit_session().unwrap().block_name,
            "EDIT_ME"
        );
    }
    assert_eq!(
        app.command_line.last_error.as_deref(),
        Some("block \"EDIT_ME\" already exists")
    );
}

#[test]
fn save_as_dialog_cancel_and_enter_follow_the_current_session() {
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    circle(&mut app, 1.0);
    let i = app.active_tab;
    let document = app.tabs[i].scene.document.clone();
    let _ = app.update(Message::DropdownSelectItem {
        dropdown_id: "BLOCK_SAVE",
        cmd: "BSAVEAS",
    });
    assert_eq!(app.active_modal, Some(ModalKind::BlockSaveAs));
    let _ = app.update(Message::BlockSaveAsName("edit_me".into()));
    assert!(app.block_save_as.as_ref().unwrap().error.is_some());
    let _ = app.update(Message::CommandEscape);
    assert!(app.block_save_as.is_none());
    assert_eq!(app.tabs[i].scene.document, document);
    let _ = app.dispatch_command("BSAVEAS");
    let _ = app.update(Message::BlockSaveAsName("COPY".into()));
    assert!(app.block_save_as.as_ref().unwrap().error.is_none());
    let _ = app.update(Message::CommandFinalize);
    assert!(app.active_modal.is_none());
    assert_eq!(
        app.tabs[i].active_block_edit_session().unwrap().block_name,
        "COPY"
    );
}

#[test]
fn save_as_preserves_base_point_description_units_and_nested_attributes() {
    use codec::entities::AttributeEntity;
    let (mut app, reference) = fixture();
    let i = app.active_tab;
    app.tabs[i]
        .scene
        .define_block_raw("CHILD", Vector3::ZERO, vec![]);
    let owner = app.tabs[i]
        .scene
        .document
        .block_records
        .get("EDIT_ME")
        .unwrap()
        .handle;
    let mut nested = Insert::new("CHILD", Vector3::new(4.0, 5.0, 0.0));
    let mut attribute = AttributeEntity::new("TAG".into(), "value".into());
    attribute.common.handle = app.tabs[i].scene.document.allocate_handle();
    nested.attributes.push(attribute);
    nested.common.owner_handle = owner;
    nested.seqend_handle = Some(app.tabs[i].scene.document.allocate_handle());
    let nested = app.tabs[i]
        .scene
        .document
        .add_entity(EntityType::Insert(nested))
        .unwrap();
    let base = Vector3::new(12.0, 34.0, 0.0);
    let record = app.tabs[i]
        .scene
        .document
        .block_records
        .get_mut("EDIT_ME")
        .unwrap();
    record.base_point = base;
    record.description = "A reusable definition".into();
    record.units = 4;
    let source_marker = record.block_entity_handle;
    if let Some(EntityType::Block(marker)) =
        app.tabs[i].scene.document.get_entity_mut(source_marker)
    {
        marker.base_point = base;
    }
    let source_insert = app.tabs[i]
        .scene
        .document
        .get_entity(nested)
        .unwrap()
        .clone();
    begin(&mut app, reference);
    assert!(app.save_block_edit_as(i, "COPY"));
    let doc = &app.tabs[i].scene.document;
    let record = doc.block_records.get("COPY").unwrap();
    assert_eq!(record.base_point, base);
    assert_eq!(record.units, 4);
    assert_eq!(record.description, "A reusable definition");
    assert_eq!(doc.get_entity(nested).unwrap(), &source_insert);
    let copied = doc
        .entities_in_block("COPY")
        .find_map(|e| match e {
            EntityType::Insert(insert) => Some(insert),
            _ => None,
        })
        .unwrap();
    let EntityType::Insert(original) = &source_insert else {
        unreachable!()
    };
    assert_eq!(copied.block_name, "CHILD");
    assert_eq!(copied.insert_point, original.insert_point);
    assert_ne!(
        copied.attributes[0].common.handle,
        original.attributes[0].common.handle
    );
    assert_eq!(
        copied.attributes[0].common.owner_handle,
        copied.common.handle
    );
    assert_ne!(copied.seqend_handle, original.seqend_handle);
    assert_eq!(copied.attributes[0].value, original.attributes[0].value);
}

#[test]
fn save_as_clones_extension_dictionaries_and_draw_order_independently() {
    use codec::objects::{
        Dictionary, ObjectType, SortEntitiesTable, XRecord, XRecordEntry, XRecordValue,
    };
    let (mut app, reference) = fixture();
    let i = app.active_tab;
    let owner = app.tabs[i]
        .scene
        .document
        .block_records
        .get("EDIT_ME")
        .unwrap()
        .handle;
    let first = members(&app)[0];
    let mut second = Line::from_points(Vector3::ZERO, Vector3::UNIT_Y);
    second.common.owner_handle = owner;
    let second = app.tabs[i]
        .scene
        .document
        .add_entity(EntityType::Line(second))
        .unwrap();
    let root = app.tabs[i]
        .scene
        .document
        .ensure_extension_dictionary(owner);
    let mut record = XRecord::new();
    record.handle = app.tabs[i].scene.document.allocate_handle();
    record.owner = root;
    record
        .entries
        .push(XRecordEntry::new(340, XRecordValue::Handle(first)));
    let leaf = record.handle;
    app.tabs[i]
        .scene
        .document
        .objects
        .insert(leaf, ObjectType::XRecord(record));
    let mut order = SortEntitiesTable::for_block(owner);
    order.handle = app.tabs[i].scene.document.allocate_handle();
    order.owner_handle = root;
    order.add_entry(first, Handle::new(0x200));
    order.send_to_back(second);
    let order_handle = order.handle;
    app.tabs[i]
        .scene
        .document
        .objects
        .insert(order_handle, ObjectType::SortEntitiesTable(order));
    let Some(ObjectType::Dictionary(dictionary)) =
        app.tabs[i].scene.document.objects.get_mut(&root)
    else {
        unreachable!()
    };
    dictionary.add_entry("DATA", leaf);
    dictionary.add_entry("ACAD_SORTENTS", order_handle);
    let original: Dictionary = dictionary.clone();
    begin(&mut app, reference);
    assert!(app.save_block_edit_as(i, "COPY"));
    let doc = &app.tabs[i].scene.document;
    let owner = doc.block_records.get("COPY").unwrap().handle;
    let copy_members = doc
        .block_records
        .get("COPY")
        .unwrap()
        .entity_handles
        .clone();
    let copied_root = doc.extension_dictionary_handle(owner).unwrap();
    assert_ne!(root, copied_root);
    assert_eq!(
        doc.objects.get(&root),
        Some(&ObjectType::Dictionary(original))
    );
    let Some(ObjectType::Dictionary(dictionary)) = doc.objects.get(&copied_root) else {
        unreachable!()
    };
    let Some(ObjectType::XRecord(record)) = doc.objects.get(&dictionary.get("DATA").unwrap())
    else {
        unreachable!()
    };
    assert_eq!(record.owner, copied_root);
    assert_eq!(
        record.entries[0].value,
        XRecordValue::Handle(copy_members[0])
    );
    let Some(ObjectType::SortEntitiesTable(order)) =
        doc.objects.get(&dictionary.get("ACAD_SORTENTS").unwrap())
    else {
        unreachable!()
    };
    assert_eq!(order.block_owner_handle, owner);
    assert!(
        order.effective_sort_handle(copy_members[1]) < order.effective_sort_handle(copy_members[0])
    );
    app.close_block_edit(i, true);
    app.undo_steps(1);
    assert!(!app.tabs[i]
        .scene
        .document
        .objects
        .contains_key(&copied_root));
    app.redo_steps(1);
    assert!(app.tabs[i]
        .scene
        .document
        .objects
        .contains_key(&copied_root));
}

#[test]
fn save_as_preserves_block_parameters_and_remaps_constraint_geometry() {
    use crate::scene::parametric_constraints::{ConstraintKind, ParametricRef, ParametricScope};
    let (mut app, reference) = fixture();
    let i = app.active_tab;
    let owner = app.tabs[i]
        .scene
        .document
        .block_records
        .get("EDIT_ME")
        .unwrap()
        .handle;
    let member = members(&app)[0];
    let set = app.tabs[i]
        .scene
        .parametric_constraint_set_mut(ParametricScope::Block(owner));
    set.local_parameters.set("width", "7").unwrap();
    set.add(
        ConstraintKind::Horizontal,
        vec![ParametricRef::whole(member)],
        None,
    );
    begin(&mut app, reference);
    app.tabs[i]
        .scene
        .parametric_constraint_set_mut(ParametricScope::Block(owner))
        .local_parameters
        .set("width", "9")
        .unwrap();
    assert!(app.save_block_edit_as(i, "COPY"));
    let new_owner = app.tabs[i]
        .scene
        .document
        .block_records
        .get("COPY")
        .unwrap()
        .handle;
    let copied = app.tabs[i]
        .scene
        .parametric_constraint_set(ParametricScope::Block(new_owner))
        .unwrap();
    assert_eq!(copied.local_parameters.resolve("width").unwrap(), 9.0);
    assert_ne!(copied.constraints[0].refs[0].entity, member);
    assert_eq!(
        app.tabs[i]
            .scene
            .parametric_constraint_set(ParametricScope::Block(owner))
            .unwrap()
            .local_parameters
            .resolve("width")
            .unwrap(),
        7.0
    );
    app.close_block_edit(i, true);
    app.undo_steps(1);
    assert!(app.tabs[i]
        .scene
        .parametric_constraint_set(ParametricScope::Block(new_owner))
        .is_none());
    app.redo_steps(1);
    assert_eq!(
        app.tabs[i]
            .scene
            .parametric_constraint_set(ParametricScope::Block(new_owner))
            .unwrap()
            .local_parameters
            .resolve("width")
            .unwrap(),
        9.0
    );
}

#[test]
fn save_as_refuses_unsupported_owned_graphs_without_losing_edits() {
    use codec::objects::{DynamicBlockObject, ObjectType};
    let (mut app, reference) = fixture();
    let i = app.active_tab;
    let mut object = DynamicBlockObject::new("BLOCKLINEARPARAMETER", "LinearParameter");
    object.handle = app.tabs[i].scene.document.allocate_handle();
    object.owner = app.tabs[i]
        .scene
        .document
        .block_records
        .get("EDIT_ME")
        .unwrap()
        .handle;
    app.tabs[i]
        .scene
        .document
        .objects
        .insert(object.handle, ObjectType::DynamicBlock(object));
    begin(&mut app, reference);
    circle(&mut app, 1.0);
    let document = app.tabs[i].scene.document.clone();
    assert!(!app.save_block_edit_as(i, "COPY"));
    assert_eq!(app.tabs[i].scene.document, document);
    assert_eq!(members(&app).len(), 2);
    assert_eq!(main_depth(&app), 0);
    assert!(app
        .command_line
        .last_error
        .as_ref()
        .unwrap()
        .contains("native object relationships"));
}

#[test]
fn save_as_retains_new_shared_layers_and_nested_definitions() {
    use codec::entities::{AttributeDefinition, AttributeEntity, MText};
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    let i = app.active_tab;
    let _ = app.dispatch_command("LAYER NEW COPY_LAYER");
    let _ = app.dispatch_command("CLAYER COPY_LAYER");
    let mut definition = AttributeDefinition::new("TAG".into(), "Prompt".into(), "first".into());
    definition.is_multiline = true;
    definition.mtext_flag = codec::entities::attribute_definition::MTextFlag::MultiLine;
    let mut text = MText::new();
    text.value = "first\\Psecond".into();
    definition.embedded_mtext = Some(Box::new(text));
    app.tabs[i].scene.define_block_raw(
        "NEW_CHILD",
        Vector3::ZERO,
        vec![EntityType::AttributeDefinition(definition)],
    );
    let source_definition = app.tabs[i]
        .scene
        .document
        .block_records
        .get("NEW_CHILD")
        .unwrap()
        .entity_handles[0];
    let mut child = Insert::new("NEW_CHILD", Vector3::ZERO);
    let mut attribute = AttributeEntity::new("TAG".into(), "first".into());
    attribute.attdef_handle = source_definition;
    child.attributes.push(attribute);
    child.common.layer = "COPY_LAYER".into();
    child.common.owner_handle = app.tabs[i].active_block_edit_session().unwrap().br_handle;
    app.tabs[i]
        .scene
        .document
        .add_entity(EntityType::Insert(child))
        .unwrap();
    assert!(app.save_block_edit_as(i, "COPY"));
    assert_eq!(app.tabs[i].active_layer, "COPY_LAYER");
    assert!(app.tabs[i]
        .scene
        .document
        .layers
        .get("COPY_LAYER")
        .is_some());
    assert!(app.tabs[i]
        .scene
        .document
        .block_records
        .get("NEW_CHILD")
        .is_some());
    assert_eq!(members(&app).len(), 1);
    let doc = &app.tabs[i].scene.document;
    let copied_definition = doc.block_records.get("NEW_CHILD").unwrap().entity_handles[0];
    assert_ne!(copied_definition, source_definition);
    let EntityType::AttributeDefinition(definition) = doc.get_entity(copied_definition).unwrap()
    else {
        unreachable!()
    };
    let text = definition.embedded_mtext.as_ref().unwrap();
    assert_eq!(text.value, "first\\Psecond");
    assert!(!text.common.handle.is_null());
    let copied_attribute = doc
        .entities_in_block("COPY")
        .find_map(|entity| match entity {
            EntityType::Insert(insert) => Some(&insert.attributes[0]),
            _ => None,
        })
        .unwrap();
    assert_eq!(copied_attribute.attdef_handle, copied_definition);
    assert!(app.tabs[i].scene.document.entities_in_block("COPY").any(|entity| matches!(entity, EntityType::Insert(insert) if insert.block_name=="NEW_CHILD" && insert.common.layer=="COPY_LAYER")));
}

#[test]
fn save_as_can_create_an_empty_definition() {
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    let i = app.active_tab;
    let original = members(&app);
    app.push_undo_snapshot(i, "Erase");
    app.tabs[i].scene.erase_entities(&original);
    app.finish_pending_history(i);
    assert!(app.save_block_edit_as(i, "EMPTY"));
    assert_eq!(members(&app), original);
    assert!(app.tabs[i]
        .scene
        .document
        .block_records
        .get("EMPTY")
        .unwrap()
        .entity_handles
        .is_empty());
    assert!(!app.block_edit_dirty(i));
}

#[test]
fn session_undo_preserves_membership_and_cannot_reach_drawing_history() {
    let (mut app, reference) = fixture();
    circle(&mut app, 50.0);
    let depth = main_depth(&app);
    begin(&mut app, reference);
    assert_eq!(main_depth(&app), depth);
    circle(&mut app, 1.0);
    assert_eq!(members(&app).len(), 2);
    app.undo_steps(1);
    assert_eq!(members(&app).len(), 1);
    assert!(!app.block_edit_dirty(app.active_tab));
    app.undo_steps(100);
    assert_eq!(main_depth(&app), depth);
    assert!(app.tabs[app.active_tab].active_block_edit.is_some());
    app.redo_steps(1);
    assert_eq!(members(&app).len(), 2);
    assert!(members(&app).iter().all(|h| app.tabs[app.active_tab]
        .scene
        .document
        .get_entity(*h)
        .is_some()));
}

#[test]
fn repeated_saves_stay_open_and_commit_one_drawing_step_each() {
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    circle(&mut app, 1.0);
    circle(&mut app, 2.0);
    let _ = app.dispatch_command("BSAVE");
    assert!(app.tabs[app.active_tab].active_block_edit.is_some());
    assert_eq!(main_depth(&app), 1);
    assert!(!app.block_edit_dirty(app.active_tab));
    let _ = app.dispatch_command("BSAVE");
    assert_eq!(main_depth(&app), 1, "a clean save is not a new edit");
    circle(&mut app, 3.0);
    let _ = app.dispatch_command("BSAVE");
    assert_eq!(main_depth(&app), 2);
    circle(&mut app, 4.0);
    let _ = app.dispatch_command("BEDIT_DISCARD");
    assert_eq!(members(&app).len(), 4, "discard restores the last save");
    assert_eq!(main_depth(&app), 2);
    assert_eq!(
        app.tabs[app.active_tab]
            .history
            .undo_stack
            .last()
            .unwrap()
            .label(),
        "Edit block EDIT_ME"
    );
    app.undo_steps(1);
    assert_eq!(members(&app).len(), 3);
    app.undo_steps(1);
    assert_eq!(members(&app).len(), 1);
    app.redo_steps(2);
    assert_eq!(members(&app).len(), 4);
}

#[test]
fn discard_preserves_drawing_redo_and_never_leaks_local_edits() {
    let (mut app, reference) = fixture();
    circle(&mut app, 50.0);
    app.undo_steps(1);
    let main_redo = app.tabs[app.active_tab].history.redo_stack.len();
    begin(&mut app, reference);
    circle(&mut app, 1.0);
    let _ = app.dispatch_command("BEDIT_DISCARD");
    assert_eq!(members(&app).len(), 1);
    assert_eq!(app.tabs[app.active_tab].history.redo_stack.len(), main_redo);
    app.redo_steps(1);
    assert_eq!(members(&app).len(), 1);
}

#[test]
fn dirty_close_cancel_discard_and_save_follow_the_checkpoint() {
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    circle(&mut app, 1.0);
    let _ = app.dispatch_command("BCLOSE");
    assert_eq!(app.active_modal, Some(ModalKind::BlockEditChanges));
    let _ = app.update(Message::BlockEditChangesCancel);
    assert!(app.tabs[app.active_tab].active_block_edit.is_some());
    assert_eq!(members(&app).len(), 2);
    let _ = app.dispatch_command("BCLOSE");
    let _ = app.update(Message::BlockEditChangesDiscard);
    assert!(app.tabs[app.active_tab].active_block_edit.is_none());
    assert_eq!(members(&app).len(), 1);
    assert_eq!(main_depth(&app), 0);
    begin(&mut app, reference);
    circle(&mut app, 2.0);
    let _ = app.dispatch_command("BCLOSE");
    let _ = app.update(Message::BlockEditChangesSave);
    assert!(app.tabs[app.active_tab].active_block_edit.is_none());
    assert_eq!(members(&app).len(), 2);
    assert_eq!(main_depth(&app), 1);
}

#[test]
fn clean_close_does_not_prompt_or_dirty_the_drawing() {
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    assert!(!app.tabs[app.active_tab].dirty);
    let _ = app.dispatch_command("BCLOSE");
    assert!(app.tabs[app.active_tab].active_block_edit.is_none());
    assert!(app.active_modal.is_none());
    assert!(!app.tabs[app.active_tab].dirty);
    assert_eq!(main_depth(&app), 0);
}

#[test]
fn dirty_switch_can_cancel_and_never_nests_sessions() {
    let (mut app, reference) = fixture();
    let i = app.active_tab;
    app.tabs[i]
        .scene
        .define_block_raw("OTHER", Vector3::ZERO, vec![]);
    let other = app.tabs[i]
        .scene
        .document
        .add_entity(EntityType::Insert(Insert::new("OTHER", Vector3::ZERO)))
        .unwrap();
    begin(&mut app, reference);
    circle(&mut app, 1.0);
    let _ = app.dispatch_command(&format!("BEDIT_BEGIN:{}", other.value()));
    assert_eq!(app.active_modal, Some(ModalKind::BlockEditChanges));
    let _ = app.update(Message::BlockEditChangesCancel);
    assert_eq!(
        app.tabs[i].active_block_edit_session().unwrap().block_name,
        "EDIT_ME"
    );
    let _ = app.dispatch_command(&format!("BEDIT_BEGIN:{}", other.value()));
    let _ = app.update(Message::BlockEditChangesSave);
    assert_eq!(app.tabs[i].block_edits.len(), 1);
    assert_eq!(
        app.tabs[i].active_block_edit_session().unwrap().block_name,
        "OTHER"
    );
    assert_eq!(main_depth(&app), 1);
}

#[test]
fn tab_close_prompts_for_block_edits_before_drawing_changes() {
    let (mut app, reference) = fixture();
    let id = app.tabs[app.active_tab].id;
    begin(&mut app, reference);
    circle(&mut app, 1.0);
    let _ = app.update(Message::TabClose(id));
    assert_eq!(app.active_modal, Some(ModalKind::BlockEditChanges));
    let _ = app.update(Message::BlockEditChangesCancel);
    assert!(app.tabs.iter().any(|t| t.id == id));
    let _ = app.update(Message::TabClose(id));
    let _ = app.update(Message::BlockEditChangesDiscard);
    assert!(!app.tabs.iter().any(|t| t.id == id));
}

#[test]
fn save_refuses_a_cycle_introduced_outside_the_insert_command() {
    let (mut app, reference) = fixture();
    let i = app.active_tab;
    app.tabs[i].scene.define_block_raw(
        "OTHER",
        Vector3::ZERO,
        vec![EntityType::Insert(Insert::new("EDIT_ME", Vector3::ZERO))],
    );
    begin(&mut app, reference);
    app.tabs[i]
        .scene
        .add_entity(EntityType::Insert(Insert::new("OTHER", Vector3::ZERO)));
    assert!(!app.save_block_edit(i));
    assert_eq!(main_depth(&app), 0);
    assert!(app.tabs[i].active_block_edit.is_some());
}

#[test]
fn drawing_history_retains_the_paper_layout_after_save_and_undo() {
    let (mut app, reference) = fixture();
    let i = app.active_tab;
    app.tabs[i].scene.add_layout("Sheet").unwrap();
    app.tabs[i].scene.set_current_layout("Sheet".into());
    begin(&mut app, reference);
    circle(&mut app, 1.0);
    let _ = app.dispatch_command("BEDIT_SAVE");
    assert_eq!(app.tabs[i].scene.current_layout, "Sheet");
    assert!(!app.tabs[i].scene.document.header.show_model_space);
    app.undo_steps(1);
    assert_eq!(app.tabs[i].scene.current_layout, "Sheet");
    assert!(!app.tabs[i].scene.document.header.show_model_space);
    assert_eq!(
        crate::io::saved_active_layout(&app.tabs[i].scene.document).as_deref(),
        Some("Sheet")
    );
    assert_eq!(members(&app).len(), 1);
    app.redo_steps(1);
    assert!(!app.tabs[i].scene.document.header.show_model_space);
    assert_eq!(members(&app).len(), 2);
}

#[test]
fn local_parameters_are_saved_and_discarded_with_the_definition() {
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    let i = app.active_tab;
    let scope = app.tabs[i].current_parametric_scope();
    app.tabs[i]
        .scene
        .parametric_constraint_set_mut(scope)
        .local_parameters
        .set("Width", "7")
        .unwrap();
    assert!(app.save_block_edit(i));
    app.tabs[i]
        .scene
        .parametric_constraint_set_mut(scope)
        .local_parameters
        .set("Width", "9")
        .unwrap();
    let _ = app.dispatch_command("BEDIT_DISCARD");
    assert_eq!(
        app.tabs[i]
            .scene
            .parametric_constraint_set(scope)
            .unwrap()
            .local_parameters
            .resolve("Width")
            .unwrap(),
        7.0
    );
    app.undo_steps(1);
    assert!(!app.tabs[i]
        .scene
        .parametric_constraint_set(scope)
        .unwrap()
        .local_parameters
        .contains("Width"));
    app.redo_steps(1);
    assert_eq!(
        app.tabs[i]
            .scene
            .parametric_constraint_set(scope)
            .unwrap()
            .local_parameters
            .resolve("Width")
            .unwrap(),
        7.0
    );
}

#[test]
fn quit_prompts_for_block_edits_before_unsaved_drawing_changes() {
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    circle(&mut app, 1.0);
    let _ = app.request_quit_with_block_edits();
    assert_eq!(app.active_modal, Some(ModalKind::BlockEditChanges));
    let _ = app.update(Message::BlockEditChangesCancel);
    assert!(app.pending_close.is_none());
    assert!(app.tabs[app.active_tab].active_block_edit.is_some());
    let _ = app.request_quit_with_block_edits();
    let _ = app.update(Message::BlockEditChangesSave);
    assert!(app.tabs[app.active_tab].active_block_edit.is_none());
    assert_eq!(app.active_modal, Some(ModalKind::Unsaved));
    assert!(matches!(app.pending_close, Some(super::PendingClose::Quit)));
}

#[test]
fn document_replacement_is_refused_until_the_editor_is_closed() {
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    assert_eq!(app.automation_op(r#"{"op":"new"}"#)["ok"], false);
    assert!(app.tabs[app.active_tab].active_block_edit.is_some());
}

#[test]
fn erasing_the_last_member_saves_an_empty_definition_and_undo_restores_it() {
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    let i = app.active_tab;
    let line = members(&app)[0];
    app.push_undo_snapshot(i, "Erase block member");
    app.tabs[i].scene.erase_entities(&[line]);
    app.finish_pending_history(i);
    assert!(members(&app).is_empty());
    app.undo_steps(1);
    assert_eq!(members(&app), vec![line]);
    app.redo_steps(1);
    assert!(members(&app).is_empty());
    let _ = app.dispatch_command("BEDIT_SAVE");
    assert!(members(&app).is_empty());
    app.undo_steps(1);
    assert_eq!(members(&app), vec![line]);
}

#[test]
fn close_all_resumes_after_discarding_a_dirty_block() {
    let (mut app, reference) = fixture();
    let first = app.tabs[app.active_tab].id;
    app.tabs.push(super::document::DocumentTab::new_drawing(2));
    let second = app.tabs.last().unwrap().id;
    begin(&mut app, reference);
    circle(&mut app, 1.0);
    let _ = app.update(Message::DocTabCloseAll);
    assert_eq!(app.active_modal, Some(ModalKind::BlockEditChanges));
    let _ = app.update(Message::BlockEditChangesDiscard);
    assert!(!app
        .tabs
        .iter()
        .any(|tab| tab.id == first || tab.id == second));
    assert!(app.pending_tab_closes.is_empty());
}

#[test]
fn explode_inside_session_edits_the_definition_and_marks_dirty() {
    let (mut app, reference) = fixture();
    let i = app.active_tab;
    app.tabs[i].scene.define_block_raw(
        "CHILD",
        Vector3::ZERO,
        vec![EntityType::Line(Line::from_points(
            Vector3::ZERO,
            Vector3::new(3.0, 0.0, 0.0),
        ))],
    );
    begin(&mut app, reference);
    let _ = app.dispatch_command("-INSERT");
    feed(&mut app, "CHILD");
    feed(&mut app, "0,5");
    feed(&mut app, "1");
    feed(&mut app, "1");
    feed(&mut app, "0");
    app.finish_pending_history(i);
    let before = members(&app);
    assert_eq!(before.len(), 2, "the nested INSERT joins the definition");
    let insert_handle = *before.last().unwrap();
    assert!(
        matches!(
            app.tabs[i].scene.document.get_entity(insert_handle),
            Some(EntityType::Insert(_))
        ),
        "expected the member just added to be the INSERT"
    );
    let depth = main_depth(&app);
    app.tabs[i].scene.select_entity(insert_handle, true);
    let _ = app.dispatch_command("EXPLODE");
    assert!(
        app.block_edit_dirty(i),
        "the explode marks the session dirty"
    );
    assert_eq!(main_depth(&app), depth, "the main stack is untouched");
    let _ = app.dispatch_command("BSAVE");
    let saved = members(&app);
    assert!(
        saved
            .iter()
            .all(|h| app.tabs[i].scene.document.get_entity(*h).is_some()),
        "the saved definition carries no dangling member handle: {saved:?}"
    );
    assert_eq!(saved.len(), 2, "the INSERT is replaced by its piece");
    assert!(
        !saved.contains(&insert_handle),
        "the exploded INSERT leaves the saved definition"
    );
    assert!(
        saved.iter().any(|h| {
            matches!(
                app.tabs[i].scene.document.get_entity(*h),
                Some(EntityType::Line(_))
            )
        }),
        "the child line lands as a member of the edited block"
    );
    let _ = app.dispatch_command("BEDIT_DISCARD");
}

#[test]
fn xclip_inside_session_stays_in_the_definition() {
    let (mut app, reference) = fixture();
    let i = app.active_tab;
    app.tabs[i].scene.define_block_raw(
        "CHILD",
        Vector3::ZERO,
        vec![EntityType::Line(Line::from_points(
            Vector3::ZERO,
            Vector3::new(3.0, 0.0, 0.0),
        ))],
    );
    begin(&mut app, reference);
    let _ = app.dispatch_command("-INSERT");
    feed(&mut app, "CHILD");
    feed(&mut app, "0,5");
    feed(&mut app, "1");
    feed(&mut app, "1");
    feed(&mut app, "0");
    app.finish_pending_history(i);
    let before = members(&app);
    let insert_handle = *before.last().unwrap();
    let depth = main_depth(&app);
    app.tabs[i].scene.select_entity(insert_handle, true);
    let _ = app.dispatch_command("XCLIP");
    feed(&mut app, "N");
    feed(&mut app, "R");
    feed(&mut app, "0,0");
    feed(&mut app, "10,10");
    app.finish_pending_history(i);
    assert!(
        crate::scene::pick::xclip::filter_handle(&app.tabs[i].scene.document, insert_handle)
            .is_some(),
        "the clip filter attaches to the in-block reference"
    );
    assert!(
        members(&app).contains(&insert_handle),
        "the clipped reference stays a member of the definition"
    );
    assert!(app.block_edit_dirty(i), "the clip marks the session dirty");
    assert_eq!(main_depth(&app), depth, "the main stack is untouched");
    let _ = app.dispatch_command("BEDIT_DISCARD");
}

#[test]
fn attdef_inside_session_lands_in_the_definition_and_undo_stays_local() {
    let (mut app, reference) = fixture();
    let i = app.active_tab;
    let depth = main_depth(&app);
    begin(&mut app, reference);
    assert_eq!(members(&app).len(), 1);
    let _ = app.dispatch_command("-ATTDEF");
    feed(&mut app, ""); // modes: done
    feed(&mut app, "TAG1");
    feed(&mut app, "prompt text");
    feed(&mut app, ""); // default value
    feed(&mut app, ""); // single line
    feed(&mut app, "0,5");
    feed(&mut app, "2");
    feed(&mut app, "0");
    app.finish_pending_history(i);
    let after = members(&app);
    assert_eq!(after.len(), 2, "the ATTDEF joins the definition");
    assert!(
        matches!(
            app.tabs[i]
                .scene
                .document
                .get_entity(*after.last().unwrap()),
            Some(EntityType::AttributeDefinition(_))
        ),
        "the new member is the ATTDEF"
    );
    assert!(
        app.block_edit_dirty(i),
        "the ATTDEF marks the session dirty"
    );
    assert_eq!(main_depth(&app), depth, "the main stack is untouched");
    app.undo_steps(1);
    assert_eq!(members(&app).len(), 1, "session undo removes the ATTDEF");
    assert_eq!(main_depth(&app), depth, "the main stack is still untouched");
    let _ = app.dispatch_command("BEDIT_DISCARD");
}

#[test]
fn save_after_session_undo_does_not_write_a_dangling_member() {
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    circle(&mut app, 1.0);
    assert_eq!(members(&app).len(), 2);
    app.undo_steps(1);
    assert_eq!(members(&app).len(), 1);
    let depth = main_depth(&app);
    let _ = app.dispatch_command("BSAVE");
    assert!(app.tabs[app.active_tab].active_block_edit.is_some());
    let saved = members(&app);
    assert!(
        saved.iter().all(|h| app.tabs[app.active_tab]
            .scene
            .document
            .get_entity(*h)
            .is_some()),
        "save after a session undo must not write a member handle whose entity is gone: {saved:?}"
    );
    assert_eq!(
        main_depth(&app),
        depth,
        "a clean save commits no new drawing step"
    );
    let _ = app.dispatch_command("BEDIT_DISCARD");
    assert_eq!(members(&app).len(), 1);
}

#[test]
fn locked_bedit_no_selection_opens_edit_block_definition_dialog() {
    let mut app = OpenCADStudio::new_for_test();
    app.automation_op(r#"{"op":"new"}"#);
    let tab = app.active_tab;
    app.tabs[tab].scene.define_block_raw(
        "DOOR",
        Vector3::ZERO,
        vec![EntityType::Line(Line::from_points(
            Vector3::ZERO,
            Vector3::UNIT_X,
        ))],
    );
    app.tabs[tab]
        .scene
        .document
        .block_records
        .get_mut("DOOR")
        .unwrap()
        .description = "Wooden door".to_string();
    app.tabs[tab]
        .scene
        .define_block_raw("WINDOW", Vector3::ZERO, vec![]);

    app.tabs[tab].scene.deselect_all();
    let _ = app.dispatch_command("BEDIT");

    assert_eq!(app.active_modal, Some(ModalKind::BlockEditPicker));
    let picker = app.block_edit_picker.as_ref().expect("picker state set");
    assert_eq!(picker.existing_names, vec!["DOOR", "WINDOW"]);
    assert_eq!(picker.name, "DOOR");
    assert_eq!(picker.description, "Wooden door");
    assert!(picker.error.is_none());
}

#[test]
fn locked_bedit_dialog_select_existing_block() {
    let mut app = OpenCADStudio::new_for_test();
    app.automation_op(r#"{"op":"new"}"#);
    let tab = app.active_tab;
    app.tabs[tab]
        .scene
        .define_block_raw("DOOR", Vector3::ZERO, vec![]);
    app.tabs[tab]
        .scene
        .define_block_raw("WINDOW", Vector3::ZERO, vec![]);

    app.tabs[tab].scene.deselect_all();
    let _ = app.dispatch_command("BEDIT");

    let _ = app.update(Message::BlockEditPickerSelect("WINDOW".to_string()));
    let picker = app.block_edit_picker.as_ref().unwrap();
    assert_eq!(picker.name, "WINDOW");

    let _ = app.update(Message::BlockEditPickerApply);
    assert_eq!(app.active_modal, None);
    assert!(app.tabs[tab].active_block_edit.is_some());
    assert_eq!(
        app.tabs[tab]
            .active_block_edit_session()
            .unwrap()
            .block_name,
        "WINDOW"
    );
}

#[test]
fn locked_bedit_dialog_name_validation() {
    let mut app = OpenCADStudio::new_for_test();
    app.automation_op(r#"{"op":"new"}"#);
    let tab = app.active_tab;
    app.tabs[tab].scene.deselect_all();
    let _ = app.dispatch_command("BEDIT");

    let _ = app.update(Message::BlockEditPickerName("INVALID/NAME".to_string()));
    assert!(app.block_edit_picker.as_ref().unwrap().error.is_some());

    let _ = app.update(Message::BlockEditPickerApply);
    assert_eq!(app.active_modal, Some(ModalKind::BlockEditPicker));
    assert!(app.tabs[tab].active_block_edit.is_none());

    let _ = app.update(Message::BlockEditPickerName("".to_string()));
    let _ = app.update(Message::BlockEditPickerApply);
    assert_eq!(app.active_modal, Some(ModalKind::BlockEditPicker));
    assert!(app.tabs[tab].active_block_edit.is_none());

    let _ = app.update(Message::CloseModal);
    assert_eq!(app.active_modal, None);
}

#[test]
fn locked_bedit_dialog_create_new_block_closes_without_saving() {
    let mut app = OpenCADStudio::new_for_test();
    app.automation_op(r#"{"op":"new"}"#);
    let tab = app.active_tab;
    app.tabs[tab].scene.deselect_all();
    let _ = app.dispatch_command("BEDIT");

    let _ = app.update(Message::BlockEditPickerName("BRAND_NEW".to_string()));
    assert!(app.block_edit_picker.as_ref().unwrap().error.is_none());

    let _ = app.update(Message::BlockEditPickerApply);
    assert_eq!(app.active_modal, None);
    assert!(app.tabs[tab].active_block_edit.is_some());
    assert_eq!(
        app.tabs[tab]
            .active_block_edit_session()
            .unwrap()
            .block_name,
        "BRAND_NEW"
    );
    assert!(!app.block_edit_dirty(tab));

    let _ = app.dispatch_command("BCLOSE");
    assert_eq!(app.active_modal, None);
    assert!(app.tabs[tab].active_block_edit.is_none());
    assert!(
        app.tabs[tab]
            .scene
            .document
            .block_records
            .get("BRAND_NEW")
            .is_none(),
        "unpersisted new block closed without saving must not remain in drawing"
    );
}

#[test]
fn locked_bedit_dialog_create_new_block_discard_dirty_leaves_no_block() {
    let mut app = OpenCADStudio::new_for_test();
    app.automation_op(r#"{"op":"new"}"#);
    let tab = app.active_tab;
    app.tabs[tab].scene.deselect_all();
    let _ = app.dispatch_command("BEDIT");

    let _ = app.update(Message::BlockEditPickerName("BRAND_NEW2".to_string()));
    let _ = app.update(Message::BlockEditPickerApply);
    assert!(app.tabs[tab].active_block_edit.is_some());

    circle(&mut app, 1.0);
    assert!(app.block_edit_dirty(tab));

    let _ = app.dispatch_command("BCLOSE");
    assert_eq!(app.active_modal, Some(ModalKind::BlockEditChanges));

    let _ = app.update(Message::BlockEditChangesDiscard);
    assert_eq!(app.active_modal, None);
    assert!(app.tabs[tab].active_block_edit.is_none());
    assert!(
        app.tabs[tab]
            .scene
            .document
            .block_records
            .get("BRAND_NEW2")
            .is_none(),
        "discarded new block must not remain in drawing"
    );
}

#[test]
fn locked_bedit_dialog_create_new_block_save_persists_and_undoes() {
    let mut app = OpenCADStudio::new_for_test();
    app.automation_op(r#"{"op":"new"}"#);
    let tab = app.active_tab;
    app.tabs[tab].scene.deselect_all();
    let _ = app.dispatch_command("BEDIT");

    let _ = app.update(Message::BlockEditPickerName("SAVED_BLOCK".to_string()));
    let _ = app.update(Message::BlockEditPickerApply);
    assert!(app.tabs[tab].active_block_edit.is_some());

    circle(&mut app, 1.0);
    let _ = app.dispatch_command("BSAVE");
    assert!(app.tabs[tab]
        .scene
        .document
        .block_records
        .get("SAVED_BLOCK")
        .is_some());

    let _ = app.dispatch_command("BCLOSE");
    assert!(app.tabs[tab].active_block_edit.is_none());
    assert!(app.tabs[tab]
        .scene
        .document
        .block_records
        .get("SAVED_BLOCK")
        .is_some());

    app.undo_steps(1);
    assert!(
        app.tabs[tab]
            .scene
            .document
            .block_records
            .get("SAVED_BLOCK")
            .is_none(),
        "undo must revert block creation"
    );

    app.redo_steps(1);
    assert!(
        app.tabs[tab]
            .scene
            .document
            .block_records
            .get("SAVED_BLOCK")
            .is_some(),
        "redo must restore block creation"
    );
}

#[test]
fn locked_bedit_with_preselected_insert_opens_directly() {
    let (mut app, reference) = fixture();
    let tab = app.active_tab;
    app.tabs[tab].scene.deselect_all();
    app.tabs[tab].scene.select_entity(reference, true);

    let _ = app.dispatch_command("BEDIT");
    assert_eq!(app.active_modal, None);
    assert!(app.tabs[tab].active_block_edit.is_some());
    assert_eq!(
        app.tabs[tab]
            .active_block_edit_session()
            .unwrap()
            .block_name,
        "EDIT_ME"
    );
}

#[test]
fn locked_bedit_clean_close_preserves_block_epoch_and_render_caches() {
    let (mut app, reference) = fixture();
    let tab = app.active_tab;
    let epoch_before = app.tabs[tab].scene.block_epoch;
    begin(&mut app, reference);
    let _ = app.dispatch_command("BCLOSE");
    assert_eq!(app.tabs[tab].scene.block_epoch, epoch_before);
}

#[test]
fn locked_bedit_discard_preserves_block_epoch_and_render_caches() {
    let (mut app, reference) = fixture();
    let tab = app.active_tab;
    let epoch_before = app.tabs[tab].scene.block_epoch;
    begin(&mut app, reference);
    circle(&mut app, 1.0);
    let _ = app.dispatch_command("BCLOSE");
    let _ = app.update(Message::BlockEditChangesDiscard);
    assert_eq!(app.tabs[tab].scene.block_epoch, epoch_before);
}

fn placed_visibility(app: &OpenCADStudio) -> &codec::objects::BlockVisibilityParameter {
    let tab = &app.tabs[app.active_tab];
    let definition = tab
        .scene
        .document
        .block_records
        .get("EDIT_ME")
        .unwrap()
        .handle;
    crate::scene::block_visibility::parameter(&tab.scene.document, definition)
        .expect("visibility parameter")
}

#[test]
fn visibility_parameter_placement_can_cancel_without_document_or_history_changes() {
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    let i = app.active_tab;
    let before = app.tabs[i].scene.document.clone();
    let history = app.tabs[i].history.undo_stack.len();
    let _ = app.dispatch_command("BPARAMETER");
    feed(&mut app, "Visibility");
    assert_eq!(
        app.tabs[i].active_cmd.as_ref().unwrap().name(),
        "BPARAMETER"
    );
    assert_eq!(app.tabs[i].scene.document, before);
    let _ = app.apply_cmd_result(crate::command::CmdResult::Cancel);
    assert_eq!(app.tabs[i].scene.document, before);
    assert_eq!(app.tabs[i].history.undo_stack.len(), history);
    assert!(!app.block_edit_dirty(i));
}

#[test]
fn visibility_parameter_placement_has_local_undo_redo_save_and_parent_undo() {
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    let i = app.active_tab;
    let before = app.tabs[i].scene.document.clone();
    let _ = app.dispatch_command("BPARAMETER VISIBILITY");
    feed(&mut app, "3,4,0");
    assert!(app.tabs[i].active_cmd.is_none());
    assert_eq!(
        placed_visibility(&app).def_point,
        Vector3::new(3.0, 4.0, 0.0)
    );
    assert_eq!(placed_visibility(&app).states[0].name, "VisibilityState0");
    assert_eq!(
        members(&app),
        before.block_records.get("EDIT_ME").unwrap().entity_handles
    );
    assert_eq!(app.tabs[i].history.undo_stack.len(), 1);
    assert!(app.block_edit_dirty(i));
    app.undo_steps(1);
    assert_eq!(app.tabs[i].scene.document.objects, before.objects);
    assert!(!app.block_edit_dirty(i));
    app.redo_steps(1);
    assert_eq!(
        placed_visibility(&app).def_point,
        Vector3::new(3.0, 4.0, 0.0)
    );
    let _ = app.dispatch_command("BSAVE");
    assert!(!app.block_edit_dirty(i));
    let _ = app.dispatch_command("BEDIT_DISCARD");
    assert_eq!(
        placed_visibility(&app).def_point,
        Vector3::new(3.0, 4.0, 0.0)
    );
    app.undo_steps(1);
    let definition = app.tabs[i]
        .scene
        .document
        .block_records
        .get("EDIT_ME")
        .unwrap()
        .handle;
    assert!(
        crate::scene::block_visibility::parameter(&app.tabs[i].scene.document, definition)
            .is_none()
    );
}

#[test]
fn visibility_parameter_placement_discard_removes_the_unsaved_graph() {
    let (mut app, reference) = fixture();
    let before = app.tabs[app.active_tab].scene.document.clone();
    begin(&mut app, reference);
    let _ = app.dispatch_command("BPARAMETER VISIBILITY");
    feed(&mut app, "7,8,0");
    let _ = app.dispatch_command("BEDIT_DISCARD");
    // Closing the editor synchronizes the restored camera to the active VPORT.
    let mut after = app.tabs[app.active_tab].scene.document.clone();
    after.vports = before.vports.clone();
    assert_eq!(after, before);
}

#[test]
fn visibility_parameter_placement_rejects_duplicates_stale_context_and_nonfinite_locations() {
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    let i = app.active_tab;
    let document_id = app.tabs[i].id;
    let definition = app.tabs[i].active_block_edit_session().unwrap().br_handle;
    let before = app.tabs[i].scene.document.clone();
    for (document_id, location) in [
        (document_id + 1, glam::DVec3::ZERO),
        (document_id, glam::DVec3::splat(f64::NAN)),
    ] {
        let _ = app.handle_place_block_visibility_parameter(document_id, definition, location);
        assert_eq!(app.tabs[i].scene.document, before);
        assert!(app.tabs[i].history.undo_stack.is_empty());
    }
    let _ = app.dispatch_command("BPARAMETER VISIBILITY");
    feed(&mut app, "3,4,0");
    let before = app.tabs[i].scene.document.clone();
    let depth = app.tabs[i].history.undo_stack.len();
    let _ = app.dispatch_command("LINE");
    let _ = app.dispatch_command("BPARAMETER VISIBILITY");
    assert_eq!(app.tabs[i].active_cmd.as_ref().unwrap().name(), "LINE");
    assert_eq!(app.tabs[i].scene.document, before);
    assert_eq!(app.tabs[i].history.undo_stack.len(), depth);
    let _ = app.dispatch_command("BTESTBLOCK");
    let test = app.active_tab;
    let before = app.tabs[test].scene.document.clone();
    let _ = app.dispatch_command("LINE");
    let _ = app.dispatch_command("BPARAMETER VISIBILITY");
    assert_eq!(app.tabs[test].active_cmd.as_ref().unwrap().name(), "LINE");
    let _ = app.handle_place_block_visibility_parameter(document_id, definition, glam::DVec3::ZERO);
    assert_eq!(app.tabs[test].scene.document, before);
}

#[test]
fn visibility_parameter_placement_preserves_an_opaque_authoring_graph() {
    use codec::objects::{DynamicBlockData, DynamicBlockObject, ObjectType};
    let (mut app, reference) = fixture();
    let i = app.active_tab;
    let doc = &mut app.tabs[i].scene.document;
    let definition = doc.block_records.get("EDIT_ME").unwrap().handle;
    let dictionary = doc.ensure_extension_dictionary(definition);
    let graph_handle = doc.allocate_handle();
    let mut graph = DynamicBlockObject::new("ACAD_EVALUATION_GRAPH", "AcDbEvalGraph");
    graph.handle = graph_handle;
    graph.owner = dictionary;
    graph.data = DynamicBlockData::Unknown;
    doc.objects
        .insert(graph_handle, ObjectType::DynamicBlock(graph));
    if let Some(ObjectType::Dictionary(dictionary)) = doc.objects.get_mut(&dictionary) {
        dictionary.add_entry("ACAD_ENHANCEDBLOCK", graph_handle);
    }
    begin(&mut app, reference);
    let before = app.tabs[i].scene.document.clone();
    for command in [
        "BPARAMETER VISIBILITY",
        "BVSTATE",
        "_BVSTATE\u{1f}NEW\u{1f}Other",
    ] {
        let _ = app.dispatch_command(command);
        assert_eq!(app.tabs[i].scene.document, before);
        assert!(app.tabs[i].history.undo_stack.is_empty());
    }
    assert!(crate::scene::block_visibility::create(
        &mut app.tabs[i].scene.document,
        definition,
        Vector3::ZERO
    )
    .is_none());
    assert_eq!(app.tabs[i].scene.document, before);
}

#[test]
fn visibility_parameter_placement_roundtrips_its_location_and_reference_grip_in_dwg_and_dxf() {
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    let _ = app.dispatch_command("BPARAMETER VISIBILITY");
    feed(&mut app, "3,4,0");
    assert_eq!(
        app.automation_op(r#"{"op":"run","cmd":"UCS ORIGIN 1,2,0"}"#)["ok"],
        true
    );
    let _ = app.dispatch_command("BSAVE");
    let _ = app.dispatch_command("BEDIT_DISCARD");
    let doc = &app.tabs[app.active_tab].scene.document;
    for extension in ["dwg", "dxf"] {
        let bytes = crate::io::save_to_bytes(doc, extension, doc.version).unwrap();
        let read = crate::io::load_bytes(&format!("visibility.{extension}"), bytes).unwrap();
        let (definition, parameter) = read
            .dynamic_visibility_for_insert(reference)
            .expect(extension);
        assert_eq!(
            read.block_records.get("EDIT_ME").unwrap().handle,
            definition
        );
        assert_eq!(parameter.def_point, Vector3::new(2.0, 2.0, 0.0));
        assert_eq!(parameter.states[0].name, "VisibilityState0");
        let grips = read
            .objects
            .values()
            .filter_map(|object| match object {
                codec::objects::ObjectType::DynamicBlock(object)
                    if object.owner == parameter.owner =>
                {
                    match &object.data {
                        codec::objects::DynamicBlockData::VisibilityGrip(grip) => {
                            Some(grip.location)
                        }
                        _ => None,
                    }
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(grips, [Vector3::new(2.0, 2.0, 0.0)], "{extension}");
    }
}

#[test]
fn visibility_parameter_placement_follows_the_editor_coordinate_frame() {
    let (mut app, reference) = fixture();
    begin(&mut app, reference);
    assert_eq!(
        app.automation_op(r#"{"op":"run","cmd":"UCS ORIGIN 100,200,300"}"#)["ok"],
        true
    );
    assert_eq!(
        app.automation_op(r#"{"op":"run","cmd":"UCS Z 90"}"#)["ok"],
        true
    );
    // BEDIT bakes UCS changes into the definition and returns to local WCS.
    assert!(app.tabs[app.active_tab].active_ucs.is_none());
    let _ = app.dispatch_command("BPARAMETER VISIBILITY");
    feed(&mut app, "3,4,0");
    assert_eq!(
        placed_visibility(&app).def_point,
        Vector3::new(3.0, 4.0, 0.0)
    );
    assert_eq!(
        app.automation_op(r#"{"op":"run","cmd":"UCS ORIGIN 1,2,0"}"#)["ok"],
        true
    );
    assert_eq!(
        placed_visibility(&app).def_point,
        Vector3::new(2.0, 2.0, 0.0)
    );
}

#[test]
fn visibility_parameter_placement_reframes_an_empty_definition_and_undoes_it() {
    let (mut app, reference) = fixture();
    let i = app.active_tab;
    let handles = app.tabs[i]
        .scene
        .document
        .block_records
        .get("EDIT_ME")
        .unwrap()
        .entity_handles
        .clone();
    for handle in handles {
        app.tabs[i].scene.document.remove_entity(handle);
    }
    begin(&mut app, reference);
    let _ = app.dispatch_command("BPARAMETER VISIBILITY");
    feed(&mut app, "3,4,0");
    let before = app.tabs[i].scene.document.objects.clone();
    assert_eq!(
        app.automation_op(r#"{"op":"run","cmd":"UCS ORIGIN 1,2,0"}"#)["ok"],
        true
    );
    assert_eq!(
        placed_visibility(&app).def_point,
        Vector3::new(2.0, 2.0, 0.0)
    );
    app.undo_steps(1);
    assert_eq!(
        placed_visibility(&app).def_point,
        Vector3::new(3.0, 4.0, 0.0)
    );
    assert_eq!(app.tabs[i].scene.document.objects, before);
    app.redo_steps(1);
    assert_eq!(
        placed_visibility(&app).def_point,
        Vector3::new(2.0, 2.0, 0.0)
    );
}
