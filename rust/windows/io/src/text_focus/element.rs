//! UI Automation element inspection and surrounding-text extraction.
use butterpollo_input_policy::text_input::{FocusedElement, Surrounding};
use windows::{
    Win32::{
        System::Ole::{SafeArrayAccessData, SafeArrayDestroy, SafeArrayUnaccessData},
        UI::Accessibility::{
            IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationValuePattern,
            TextPatternRangeEndpoint_End, TextPatternRangeEndpoint_Start, TextUnit_Character,
            UIA_TextPatternId, UIA_ValuePatternId,
        },
    },
    core::BSTR,
};

/// What the focus watcher needs to know about `element`, from its cached properties.
pub(super) unsafe fn describe(element: &IUIAutomationElement) -> FocusedElement {
    let string = |value: windows::core::Result<BSTR>| {
        value.map(|value| value.to_string()).unwrap_or_default()
    };
    // SAFETY: the caller holds `element` on its COM thread, built with the watcher's cache.
    unsafe {
        let value = element
            .GetCachedPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            .ok();
        FocusedElement {
            control_type: element
                .CachedControlType()
                .map(|kind| kind.0)
                .unwrap_or_default(),
            enabled: element
                .CachedIsEnabled()
                .is_ok_and(|enabled| enabled.as_bool()),
            password: element
                .CachedIsPassword()
                .is_ok_and(|password| password.as_bool()),
            read_only: value
                .and_then(|value| value.CachedIsReadOnly().ok())
                .map(|read_only| read_only.as_bool()),
            text_pattern: element
                .GetCachedPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
                .is_ok(),
            aria_role: string(element.CachedAriaRole()),
            name: string(element.CachedName()),
            automation_id: string(element.CachedAutomationId()),
            class_name: string(element.CachedClassName()),
            localized_type: string(element.CachedLocalizedControlType()),
            in_spinner: false,
        }
    }
}

/// The UI Automation runtime id of `element`, cached with the focused element and read only
/// when the focus moves to another field, on the watcher's thread.
pub(super) unsafe fn runtime_id(element: &IUIAutomationElement) -> Option<Vec<i32>> {
    // SAFETY: the caller holds `element` on its COM thread; the array UI Automation returns
    // is ours, read while locked and destroyed once.
    unsafe {
        let array = element.GetRuntimeId().ok()?;
        if array.is_null() {
            return None;
        }
        let ids = (|| {
            if (*array).cDims != 1 || (*array).cbElements as usize != size_of::<i32>() {
                return None;
            }
            let len = usize::try_from((*array).rgsabound[0].cElements).ok()?;
            let mut data = std::ptr::null_mut();
            SafeArrayAccessData(array, &mut data).ok()?;
            let ids = std::slice::from_raw_parts(data.cast::<i32>(), len).to_vec();
            let _ = SafeArrayUnaccessData(array);
            Some(ids)
        })();
        let _ = SafeArrayDestroy(array);
        ids
    }
}

/// The text around the cursor of `element`: up to `max_text` characters either side of the
/// selection from its text pattern, else its whole value without a selection. Core cuts it
/// to `max_text` bytes.
pub(super) unsafe fn read_text(
    element: &IUIAutomationElement,
    max_text: usize,
) -> Option<Surrounding> {
    let characters = i32::try_from(max_text).unwrap_or(i32::MAX);
    // SAFETY: the caller holds `element` on its COM thread.
    unsafe {
        if let Ok(pattern) =
            element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            && let Some(surrounding) = around_selection(&pattern, characters)
        {
            return Some(surrounding);
        }
        let value = element
            .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            .ok()?;
        Some(Surrounding {
            text: value.CurrentValue().ok()?.to_string(),
            selection: None,
        })
    }
}

unsafe fn around_selection(
    pattern: &IUIAutomationTextPattern,
    characters: i32,
) -> Option<Surrounding> {
    // SAFETY: the caller holds `pattern` on its COM thread.
    unsafe {
        let ranges = pattern.GetSelection().ok()?;
        if ranges.Length().ok()? < 1 {
            return None;
        }
        let selection = ranges.GetElement(0).ok()?;
        let before = selection.Clone().ok()?;
        before
            .MoveEndpointByRange(
                TextPatternRangeEndpoint_End,
                &selection,
                TextPatternRangeEndpoint_Start,
            )
            .ok()?;
        before
            .MoveEndpointByUnit(
                TextPatternRangeEndpoint_Start,
                TextUnit_Character,
                -characters,
            )
            .ok()?;
        let after = selection.Clone().ok()?;
        after
            .MoveEndpointByRange(
                TextPatternRangeEndpoint_Start,
                &selection,
                TextPatternRangeEndpoint_End,
            )
            .ok()?;
        after
            .MoveEndpointByUnit(TextPatternRangeEndpoint_End, TextUnit_Character, characters)
            .ok()?;
        let before = before.GetText(characters).ok()?.to_string();
        // A selection longer than `characters` is cut; the window core keeps then starts at
        // the selection and ends inside it, so the text after it never shows.
        let selected = selection.GetText(characters).ok()?.to_string();
        let after = after.GetText(characters).ok()?.to_string();
        let start = before.encode_utf16().count();
        let end = start + selected.encode_utf16().count();
        Some(Surrounding {
            text: before + &selected + &after,
            selection: Some((start, end)),
        })
    }
}
