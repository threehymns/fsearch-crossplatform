//! Thread QoS tags. Upstream sets Apple QoS classes directly wherever a
//! thread is born; Linux has no QoS tiers, so these compile to nothing
//! there. One paired definition per class keeps every call site identical
//! on both OSes: when upstream adds a QoS call, the Linux port swaps the
//! call for the matching one here instead of re-deriving cfg gates at each
//! site. (Deliberately QoS only: background-pool *scheduling* parity, if
//! ever wanted, is a separate, measured decision.)

#[cfg(target_os = "macos")]
pub fn user_interactive() {
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
}

#[cfg(target_os = "macos")]
pub fn user_initiated() {
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INITIATED, 0);
    }
}

#[cfg(target_os = "macos")]
pub fn utility() {
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn user_interactive() {}

#[cfg(not(target_os = "macos"))]
pub fn user_initiated() {}

#[cfg(not(target_os = "macos"))]
pub fn utility() {}
