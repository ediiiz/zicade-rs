//! The window's animations follow Windows' "Show animations" setting, so the
//! setting must be readable wherever the window runs.

#[cfg(windows)]
#[test]
fn the_animation_setting_is_readable_on_windows() {
    assert!(
        zicade_win::animations_enabled().is_some(),
        "SystemParametersInfo(SPI_GETCLIENTAREAANIMATION) must answer"
    );
}

#[cfg(not(windows))]
#[test]
fn the_animation_setting_is_unknown_off_windows() {
    assert_eq!(zicade_win::animations_enabled(), None);
}
