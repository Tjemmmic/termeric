//! Terminal methods for the xterm keyboard resources, keyboard types, and
//! user-defined keys.

use super::*;

impl Terminal {
    /// Get xterm's terminal-local resource-zero `modifyKeyboard` and
    /// `formatKeyboard` state.
    pub fn xterm_keyboard_resource(&self) -> XtermKeyboardResource {
        self.xterm_keyboard_resource
    }

    /// Get the global xterm-compatible keyboard type selected by private mode
    /// 1060 or 1061.
    pub fn xterm_keyboard_type(&self) -> XtermKeyboardType {
        self.xterm_keyboard_type
    }

    /// Get the configured keyboard type restored by RIS.
    pub fn initial_xterm_keyboard_type(&self) -> XtermKeyboardType {
        self.initial_xterm_keyboard_type
    }

    /// Get the decoded DECUDK definition for a DEC function-key code.
    pub fn xterm_user_defined_key(&self, code: u8) -> Option<&[u8]> {
        self.xterm_user_defined_keys.get(code)
    }

    /// Iterate over defined DECUDK entries in ascending DEC key-code order.
    pub fn xterm_user_defined_keys(&self) -> impl Iterator<Item = (u8, &[u8])> {
        self.xterm_user_defined_keys.iter()
    }

    #[cfg(test)]
    pub(crate) fn xterm_user_defined_key_bytes(&self) -> usize {
        self.xterm_user_defined_keys.bytes()
    }

    pub(crate) fn clear_xterm_user_defined_keys(&mut self) {
        self.xterm_user_defined_keys.clear();
    }

    pub(crate) fn define_xterm_user_defined_key(&mut self, code: u16, definition: Vec<u8>) {
        self.xterm_user_defined_keys.define(code, definition);
    }

    /// Change the configured RIS baseline. Callers applying a changed live
    /// configuration may also replace the current application-selected type;
    /// callers merely propagating policy to an existing owner can preserve it.
    pub fn configure_xterm_keyboard_type(
        &mut self,
        keyboard_type: XtermKeyboardType,
        apply_current: bool,
    ) {
        self.initial_xterm_keyboard_type = keyboard_type;
        if apply_current {
            self.xterm_keyboard_type = keyboard_type;
            self.saved_xterm_keyboard_type = None;
        }
    }

    /// Apply xterm's mutually exclusive keyboard-type private modes. Resetting
    /// either mode returns to the normal type, even if the other type was
    /// active, matching xterm's `set_keyboard_type` behavior.
    pub(crate) fn set_xterm_keyboard_type(
        &mut self,
        keyboard_type: XtermKeyboardType,
        enable: bool,
    ) {
        self.xterm_keyboard_type = if enable {
            keyboard_type
        } else {
            XtermKeyboardType::Normal
        };
    }

    pub(crate) fn save_xterm_keyboard_type(&mut self) {
        self.saved_xterm_keyboard_type = Some(self.xterm_keyboard_type);
    }

    pub(crate) fn restore_xterm_keyboard_type(&mut self) {
        self.xterm_keyboard_type = self
            .saved_xterm_keyboard_type
            .unwrap_or(self.initial_xterm_keyboard_type);
    }

    pub(crate) fn set_xterm_keyboard_modify(&mut self, parameter: u16) {
        self.xterm_keyboard_resource = self.xterm_keyboard_resource.with_modify(parameter);
    }

    pub(crate) fn reset_xterm_keyboard_modify(&mut self) {
        self.xterm_keyboard_resource = self
            .xterm_keyboard_resource
            .with_modify(XtermKeyboardResource::default().modify());
    }

    pub(crate) fn report_xterm_keyboard_modify(&mut self) {
        self.push_event(TerminalEvent::PtyWrite(
            format!("\x1b[>0;{}m", self.xterm_keyboard_resource.modify()).into_bytes(),
        ));
    }

    pub(crate) fn set_xterm_keyboard_format(&mut self, parameter: u16) {
        self.xterm_keyboard_resource = self.xterm_keyboard_resource.with_format(parameter);
    }

    pub(crate) fn reset_xterm_keyboard_format(&mut self) {
        self.xterm_keyboard_resource = self
            .xterm_keyboard_resource
            .with_format(XtermKeyboardResource::default().format());
    }

    pub(crate) fn report_xterm_keyboard_format(&mut self) {
        self.push_event(TerminalEvent::PtyWrite(
            format!("\x1b[>0;{}f", self.xterm_keyboard_resource.format()).into_bytes(),
        ));
    }

    /// One of xterm's terminal-local mode-and-format key resources.
    pub fn xterm_key_resource(&self, resource: XtermKeyResource) -> XtermKeySetting {
        self.xterm_key_resources.get(resource)
    }

    /// All of xterm's terminal-local mode-and-format key resources.
    pub fn xterm_key_resources(&self) -> XtermKeyResources {
        self.xterm_key_resources
    }

    pub(crate) fn set_xterm_key_mode(&mut self, resource: XtermKeyResource, parameter: u16) {
        self.xterm_key_resources.set_mode(resource, parameter);
    }

    pub(crate) fn reset_xterm_key_mode(&mut self, resource: XtermKeyResource) {
        self.xterm_key_resources.reset_mode(resource);
    }

    pub(crate) fn report_xterm_key_mode(&mut self, resource: XtermKeyResource) {
        let mode = self.xterm_key_resources.get(resource).mode().parameter();
        self.push_event(TerminalEvent::PtyWrite(
            format!("\x1b[>{};{mode}m", resource.number()).into_bytes(),
        ));
    }

    pub(crate) fn set_xterm_key_format(&mut self, resource: XtermKeyResource, parameter: u16) {
        self.xterm_key_resources.set_format(resource, parameter);
    }

    pub(crate) fn reset_xterm_key_format(&mut self, resource: XtermKeyResource) {
        self.xterm_key_resources.reset_format(resource);
    }

    pub(crate) fn report_xterm_key_format(&mut self, resource: XtermKeyResource) {
        let format = self.xterm_key_resources.get(resource).format().parameter();
        self.push_event(TerminalEvent::PtyWrite(
            format!("\x1b[>{};{format}f", resource.number()).into_bytes(),
        ));
    }

    /// Get xterm's terminal-local `modifyOtherKeys` policy.
    pub fn xterm_modify_other_keys(&self) -> XtermModifyOtherKeys {
        self.xterm_modify_other_keys
    }

    pub(crate) fn set_xterm_modify_other_keys(&mut self, level: u16, modifier_mask: u16) {
        self.xterm_modify_other_keys.set(level, modifier_mask);
    }

    pub(crate) fn report_xterm_modify_other_keys(&mut self) {
        self.push_event(TerminalEvent::PtyWrite(
            format!("\x1b[>4;{}m", self.xterm_modify_other_keys.level()).into_bytes(),
        ));
    }

    pub(crate) fn set_xterm_other_key_format(&mut self, parameter: u16) {
        self.xterm_modify_other_keys.set_format(parameter);
    }

    pub(crate) fn report_xterm_other_key_format(&mut self) {
        self.push_event(TerminalEvent::PtyWrite(
            format!(
                "\x1b[>4;{}f",
                self.xterm_modify_other_keys.format().parameter()
            )
            .into_bytes(),
        ));
    }
}
