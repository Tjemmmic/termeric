//! State for the chrome around the terminals: modals, pickers, panels,
//! menus, toasts, and the agent view, plus what they share (animation,
//! text input and layout, geometry, color, icons, and modal stacking).
//! The renderer draws from snapshots of this state.

pub mod age;
pub mod agent_view;
pub mod agents_panel;
pub mod animation;
pub mod color;
pub mod command_palette;
pub mod commands;
pub mod constants;
pub mod context_picker;
pub mod cursor_motion;
pub mod diff_review;
pub mod domain_picker;
pub mod filter_field;
pub mod focus_step;
pub mod geometry;
pub mod guardrail;
pub mod icon;
pub mod info_overlay;
pub mod modal;
pub mod popup_menu;
pub mod request_scope;
pub mod scroll;
pub mod search_bar;
pub mod session_panel;
pub mod settings;
pub mod shortcuts_panel;
pub mod side_panel;
pub mod text;
pub mod text_area;
pub mod text_card;
pub mod text_input;
pub mod text_input_viewport;
pub mod text_layout;
pub mod toast;
pub mod unicode_picker;
pub mod workspace_menu;
