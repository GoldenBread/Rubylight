//! UI Automation event handler and focus/text watcher state machine.
use super::{
    WAKE_FOCUS, WAKE_TEXT,
    element::{describe, read_text, runtime_id},
    post,
};
use anyhow::{Context, Result};
use butterpollo_input_policy::text_input::{self, Field, Focused, Watch};
use windows::{
    Win32::{
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
        UI::Accessibility::{
            CUIAutomation, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationElement,
            IUIAutomationEventHandler, IUIAutomationEventHandler_Impl,
            IUIAutomationFocusChangedEventHandler, IUIAutomationFocusChangedEventHandler_Impl,
            TreeScope_Element, UIA_AriaRolePropertyId, UIA_AutomationIdPropertyId,
            UIA_ClassNamePropertyId, UIA_ControlTypePropertyId, UIA_EVENT_ID,
            UIA_IsEnabledPropertyId, UIA_IsPasswordPropertyId, UIA_LocalizedControlTypePropertyId,
            UIA_NamePropertyId, UIA_ProcessIdPropertyId, UIA_RuntimeIdPropertyId,
            UIA_Text_TextChangedEventId, UIA_Text_TextSelectionChangedEventId, UIA_TextPatternId,
            UIA_ValueIsReadOnlyPropertyId, UIA_ValuePatternId,
        },
    },
    core::{HRESULT, Ref, implement},
};

/// The text events followed in a field whose text is read.
const TEXT_EVENTS: [UIA_EVENT_ID; 2] = [
    UIA_Text_TextChangedEventId,
    UIA_Text_TextSelectionChangedEventId,
];

/// Wakes the watcher thread from UI Automation's.
#[implement(IUIAutomationFocusChangedEventHandler, IUIAutomationEventHandler)]
struct Wake(u32);

impl IUIAutomationFocusChangedEventHandler_Impl for Wake_Impl {
    fn HandleFocusChangedEvent(
        &self,
        _sender: Ref<IUIAutomationElement>,
    ) -> windows::core::Result<()> {
        post(self.0, WAKE_FOCUS);
        Ok(())
    }
}

impl IUIAutomationEventHandler_Impl for Wake_Impl {
    fn HandleAutomationEvent(
        &self,
        _sender: Ref<IUIAutomationElement>,
        _event: UIA_EVENT_ID,
    ) -> windows::core::Result<()> {
        post(self.0, WAKE_TEXT);
        Ok(())
    }
}

pub(super) struct Watcher {
    automation: IUIAutomation,
    cache: IUIAutomationCacheRequest,
    focus_handler: IUIAutomationFocusChangedEventHandler,
    text_handler: IUIAutomationEventHandler,
    max_text: usize,
    /// The focused text field.
    field: Option<(IUIAutomationElement, Field)>,
    /// The focused text field's id ([`Focused::element`]), from its runtime id each time the
    /// focus moves to another one.
    element: u32,
    /// The field whose text events are followed.
    followed: Option<IUIAutomationElement>,
    /// Whether the focus handler is registered: while the watch is on.
    pub(super) registered: bool,
}

impl Watcher {
    pub(super) unsafe fn new(thread: u32) -> Result<Self> {
        // SAFETY: the caller initialized COM on this thread.
        unsafe {
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
                    .context("UI Automation is not available")?;
            let cache = automation.CreateCacheRequest()?;
            for property in [
                UIA_ControlTypePropertyId,
                UIA_IsEnabledPropertyId,
                UIA_IsPasswordPropertyId,
                UIA_ValueIsReadOnlyPropertyId,
                UIA_AriaRolePropertyId,
                UIA_NamePropertyId,
                UIA_AutomationIdPropertyId,
                UIA_ClassNamePropertyId,
                UIA_LocalizedControlTypePropertyId,
                UIA_RuntimeIdPropertyId,
                UIA_ProcessIdPropertyId,
            ] {
                cache.AddProperty(property)?;
            }
            cache.AddPattern(UIA_ValuePatternId)?;
            cache.AddPattern(UIA_TextPatternId)?;
            let wake = Wake(thread);
            let focus_handler: IUIAutomationFocusChangedEventHandler = wake.into();
            let text_handler: IUIAutomationEventHandler =
                windows::core::Interface::cast(&focus_handler)?;
            Ok(Self {
                automation,
                cache,
                focus_handler,
                text_handler,
                max_text: 0,
                field: None,
                element: 0,
                followed: None,
                registered: false,
            })
        }
    }

    /// Pauses for [`Watch::Off`]: no handlers and no field, so the next `On` reads the id of
    /// the field it finds again. `On` follows the focus again and reports it.
    pub(super) unsafe fn apply(
        &mut self,
        watch: Watch,
        report: &dyn Fn(Option<Focused>),
    ) -> Result<()> {
        // SAFETY: on the watcher's thread, with COM initialized.
        unsafe {
            match watch {
                Watch::Off => {
                    self.follow(None);
                    if self.registered {
                        self.registered = false;
                        let _ = self
                            .automation
                            .RemoveFocusChangedEventHandler(&self.focus_handler);
                    }
                    self.field = None;
                }
                Watch::On { max_text } => {
                    self.max_text = max_text;
                    if !self.registered {
                        self.automation
                            .AddFocusChangedEventHandler(&self.cache, &self.focus_handler)
                            .context("UI Automation does not report focus changes")?;
                        self.registered = true;
                    }
                    self.focus_moved(report);
                }
            }
            Ok(())
        }
    }

    /// Whether `element`, an edit, is the text box of a number box: its parent is a spinner.
    /// A parent that cannot be read counts as none, so the field stays plain text.
    unsafe fn in_spinner(&self, element: &IUIAutomationElement) -> bool {
        // SAFETY: on the watcher's thread, with COM initialized; `element` is from it.
        unsafe {
            match self
                .automation
                .ControlViewWalker()
                .and_then(|walker| walker.GetParentElement(element))
            {
                Ok(parent) => parent
                    .CurrentControlType()
                    .is_ok_and(|kind| kind.0 == text_input::control_type::SPINNER),
                Err(error) => {
                    tracing::debug!(%error, "the parent of the focused edit could not be read");
                    false
                }
            }
        }
    }

    /// Reads the newest focus and reports it.
    pub(super) unsafe fn focus_moved(&mut self, report: &dyn Fn(Option<Focused>)) {
        // SAFETY: on the watcher's thread, with COM initialized.
        unsafe {
            let focused = match self.automation.GetFocusedElementBuildCache(&self.cache) {
                Ok(element) => {
                    let mut described = describe(&element);
                    if described.control_type == text_input::control_type::EDIT {
                        described.in_spinner = self.in_spinner(&element);
                    }
                    let own = element
                        .CachedProcessId()
                        .is_ok_and(|process| u32::try_from(process) == Ok(std::process::id()));
                    if text_input::passes_over(&described, own) {
                        tracing::debug!(
                            control_type = described.control_type,
                            class = %described.class_name,
                            own,
                            "a focus event that leaves the focus where it was"
                        );
                        return;
                    }
                    Some((element, described))
                }
                // The app is busy or the element went away as it was read: the next focus
                // event says where the focus went, and a lower and raise now would flap.
                Err(error) if passing(error.code()) => {
                    tracing::debug!(%error, "the focus could not be read for now");
                    return;
                }
                // The focus may be nowhere, or in an app that UI Automation cannot reach (a
                // secure desktop, or an elevated app under a host that is not): no text field.
                Err(error) => {
                    if self.field.is_some() {
                        tracing::info!(%error, "the focus left the text field for one UI Automation cannot read");
                    }
                    None
                }
            };
            if self.field.is_some()
                && let Some((element, described)) = &focused
                && text_input::classify(described).is_none()
            {
                // Says what took the focus when the keyboard lowers without the user.
                tracing::info!(
                    control_type = described.control_type,
                    class = %described.class_name,
                    process = element.CachedProcessId().unwrap_or_default(),
                    "the focus left the text field"
                );
            }
            let field = focused.and_then(|(element, described)| {
                let field = text_input::classify(&described)?;
                Some((element, field))
            });
            let read = field
                .as_ref()
                .filter(|(_, field)| self.max_text > 0 && !field.sensitive)
                .map(|(element, _)| element.clone());
            self.follow(read);
            // Focus events also come for the element that has it; only another one counts.
            let same = match (&self.field, &field) {
                (Some((before, _)), Some((element, _))) => self
                    .automation
                    .CompareElements(before, element)
                    .is_ok_and(|same| same.as_bool()),
                _ => false,
            };
            if let Some((element, _)) = &field
                && !same
            {
                self.element = match runtime_id(element) {
                    Some(runtime_id) => text_input::element_id(&runtime_id),
                    None => {
                        // Still another element: the client hears the move, but cannot tell
                        // this field from one it closed the keyboard on before.
                        tracing::debug!("the focused field has no UI Automation runtime id");
                        self.element.wrapping_add(1).max(1)
                    }
                };
            }
            self.field = field;
            self.report(report);
        }
    }

    /// Reports the followed field again with its new text.
    pub(super) unsafe fn text_changed(&mut self, report: &dyn Fn(Option<Focused>)) {
        // SAFETY: on the watcher's thread, with COM initialized.
        unsafe {
            if self.followed.is_some() {
                self.report(report);
            }
        }
    }

    unsafe fn report(&self, report: &dyn Fn(Option<Focused>)) {
        // SAFETY: on the watcher's thread, with COM initialized.
        unsafe {
            report(self.field.as_ref().map(|(element, field)| {
                Focused {
                    field: *field,
                    element: self.element,
                    surrounding: (self.max_text > 0 && !field.sensitive)
                        .then(|| read_text(element, self.max_text))
                        .flatten(),
                }
            }));
        }
    }

    /// Follows the text events of `element` only, or of none.
    unsafe fn follow(&mut self, element: Option<IUIAutomationElement>) {
        // SAFETY: on the watcher's thread, with COM initialized.
        unsafe {
            let same = match (&self.followed, &element) {
                (Some(followed), Some(element)) => self
                    .automation
                    .CompareElements(followed, element)
                    .is_ok_and(|same| same.as_bool()),
                (None, None) => true,
                _ => false,
            };
            if same {
                return;
            }
            if let Some(followed) = self.followed.take() {
                for event in TEXT_EVENTS {
                    let _ = self.automation.RemoveAutomationEventHandler(
                        event,
                        &followed,
                        &self.text_handler,
                    );
                }
            }
            if let Some(element) = element {
                for event in TEXT_EVENTS {
                    if let Err(error) = self.automation.AddAutomationEventHandler(
                        event,
                        &element,
                        TreeScope_Element,
                        None,
                        &self.text_handler,
                    ) {
                        // The field's text still goes out with each focus change.
                        tracing::debug!(%error, "the focused field does not report text changes");
                    }
                }
                self.followed = Some(element);
            }
        }
    }

    pub(super) unsafe fn stop(&mut self) {
        // SAFETY: on the watcher's thread, with COM initialized.
        unsafe {
            self.follow(None);
            let _ = self
                .automation
                .RemoveFocusChangedEventHandler(&self.focus_handler);
            let _ = self.automation.RemoveAllEventHandlers();
        }
    }
}

/// Whether UI Automation failed to read the focus only for now: the app took too long or
/// refused the call while busy, or the element went away as it was read.
pub(super) fn passing(error: HRESULT) -> bool {
    const UIA_E_ELEMENTNOTAVAILABLE: HRESULT = HRESULT(0x8004_0201_u32 as i32);
    const UIA_E_TIMEOUT: HRESULT = HRESULT(0x8013_1505_u32 as i32);
    const RPC_E_CALL_REJECTED: HRESULT = HRESULT(0x8001_0001_u32 as i32);
    const RPC_E_SERVERCALL_RETRYLATER: HRESULT = HRESULT(0x8001_010A_u32 as i32);
    [
        UIA_E_ELEMENTNOTAVAILABLE,
        UIA_E_TIMEOUT,
        RPC_E_CALL_REJECTED,
        RPC_E_SERVERCALL_RETRYLATER,
    ]
    .contains(&error)
}
