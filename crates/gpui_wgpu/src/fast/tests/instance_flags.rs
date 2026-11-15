use super::{native_instance_flags, platform_flags};
use wgpu::InstanceFlags;

#[test]
fn android_disables_driver_debug_utils_without_disabling_validation() {
    let flags = InstanceFlags::debugging() | InstanceFlags::DISCARD_HAL_LABELS;
    let actual = platform_flags(flags, true);
    assert!(!actual.contains(InstanceFlags::DEBUG));
    assert!(actual.contains(InstanceFlags::VALIDATION));
    assert!(actual.contains(InstanceFlags::VALIDATION_INDIRECT_CALL));
    assert_eq!(actual, flags - InstanceFlags::DEBUG);
}

#[test]
fn release_android_and_other_platforms_preserve_unrelated_flags() {
    assert_eq!(
        platform_flags(InstanceFlags::empty(), true),
        InstanceFlags::empty()
    );
    assert_eq!(
        platform_flags(InstanceFlags::all(), false),
        InstanceFlags::all()
    );
}

#[test]
fn native_configuration_uses_target_policy() {
    assert_eq!(
        native_instance_flags(),
        platform_flags(InstanceFlags::default(), cfg!(target_os = "android")),
    );
}
