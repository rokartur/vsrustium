//! The host's color scheme as Electron's `nativeTheme` reports it to hostColorSchemeService.ts on macOS:
//! high contrast is the Increase Contrast accessibility setting, dark the system appearance.

use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSString, NSUserDefaults};

/// `nativeTheme.shouldUseHighContrastColors`: System Settings > Accessibility > Display > Increase contrast.
pub fn high_contrast() -> bool {
    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldIncreaseContrast()
}

/// `nativeTheme.shouldUseDarkColors` before the app has an appearance: the global `AppleInterfaceStyle` default.
pub fn dark() -> bool {
    let style = NSUserDefaults::standardUserDefaults().stringForKey(&NSString::from_str("AppleInterfaceStyle"));
    style.is_some_and(|style| style.to_string() == "Dark")
}

/// themeConfiguration.ts `getPreferredColorScheme` with `window.autoDetectHighContrast`: the setting of the
/// preferred high contrast theme to use, when the host is in high contrast.
pub fn preferred_high_contrast_setting(detect_high_contrast: bool, high_contrast: bool, dark: bool) -> Option<&'static str> {
    if !detect_high_contrast || !high_contrast {
        return None;
    }
    Some(if dark { "workbench.preferredHighContrastColorTheme" } else { "workbench.preferredHighContrastLightColorTheme" })
}

#[cfg(test)]
mod tests {
    use super::preferred_high_contrast_setting;

    #[test]
    fn high_contrast_hosts_pick_the_preferred_theme_of_their_appearance() {
        assert_eq!(preferred_high_contrast_setting(true, true, true), Some("workbench.preferredHighContrastColorTheme"));
        assert_eq!(preferred_high_contrast_setting(true, true, false), Some("workbench.preferredHighContrastLightColorTheme"));
        assert_eq!(preferred_high_contrast_setting(true, false, true), None);
        assert_eq!(preferred_high_contrast_setting(false, true, true), None);
    }
}
