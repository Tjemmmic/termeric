//! Desktop notifications on macOS, through the UserNotifications framework.
//!
//! The notification centre is one per process, but every window has its own
//! notification worker and backend. So the centre, its delegate, and its
//! button categories are shared process-wide (`CENTER`), and the delegate
//! hands each response to the worker whose session named the notification
//! (`ROUTES`). The pieces that decide what a notification shows and what a
//! response means are plain functions, tested without the framework; nothing
//! here touches the centre until a notification is shown from inside
//! Termeric.app, because without an app bundle the centre raises an
//! Objective-C exception, which aborts.

use super::icon::{DecodedNotificationIcon, resolve_transmitted_icon};
use super::tracked::{
    DEFAULT_ACTION_KEY, TrackedCommand, TrackedNotificationBackend, without_bidi_formatting,
};
use block2::{DynBlock, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AnyThread, available, define_class, msg_send};
use objc2_foundation::{NSArray, NSBundle, NSDictionary, NSError, NSSet, NSString, NSURL};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNErrorCode, UNErrorDomain, UNMutableNotificationContent,
    UNNotification, UNNotificationAction, UNNotificationActionOptions, UNNotificationAttachment,
    UNNotificationCategory, UNNotificationCategoryOptions, UNNotificationContent,
    UNNotificationDefaultActionIdentifier, UNNotificationDismissActionIdentifier,
    UNNotificationInterruptionLevel, UNNotificationPresentationOptions, UNNotificationRequest,
    UNNotificationResponse, UNNotificationSound, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};
use std::collections::VecDeque;
use std::io::Write;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Duration;
use termeric_core::{NotificationRequest, NotificationSound, NotificationUrgency};

/// What a person sees when Termeric runs outside its app bundle (`cargo run`,
/// a test binary): the centre needs a bundle identifier.
const NEEDS_APP_BUNDLE: &str = "notifications need Termeric.app, and this Termeric runs outside it";
/// What a person sees when they have not allowed Termeric's notifications.
const NOT_ALLOWED: &str =
    "Termeric is not allowed to show notifications; allow them in System Settings → Notifications";
/// How long the worker waits for the centre to answer one request. The
/// centre answers on its own queue; a request it never answers must not hold
/// the worker, which also keeps every other notification's lifecycle.
const CENTER_REPLY_TIMEOUT: Duration = Duration::from_secs(2);
/// Random bytes in a backend's session, which keeps one window's
/// notifications, and those of earlier runs, from answering for another's.
const SESSION_BYTES: usize = 8;
const NOTIFICATION_IDENTIFIER_PREFIX: &str = "termeric-";
/// A button's identifier within its category: its 1-based position.
const CATEGORY_BUTTON_PREFIX: &str = "button-";
/// The `userInfo` key holding a notification's own button action keys, so a
/// button press answers for the revision of the notification it was on.
const BUTTON_ACTION_KEYS_KEY: &str = "termeric-button-action-keys";
/// Button sets registered with the centre at once, for the whole process.
/// A tracked notification shows one set, and a window tracks at most 64, so
/// this covers every live notification of one window. Past it the least
/// recently shown set goes first, and notifications still showing it lose
/// their buttons and dismissal reports.
const MAX_REGISTERED_CATEGORIES: usize = 64;
const ICON_ATTACHMENT_IDENTIFIER: &str = "icon";

/// The text and presentation of a notification, worked out from a request
/// without the framework.
#[derive(Debug, PartialEq, Eq)]
struct NotificationContent {
    title: String,
    body: String,
    /// The person's notification sound, or none.
    plays_sound: bool,
    /// `None` keeps the system's default level.
    interruption_level: Option<InterruptionLevel>,
    buttons: ButtonSet,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InterruptionLevel {
    Passive,
    Active,
    TimeSensitive,
}

/// The buttons a notification shows: the labels, in order, and whether
/// pressing one brings Termeric to the front. A category is registered per
/// distinct set.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ButtonSet {
    labels: Vec<String>,
    foreground: bool,
}

fn notification_content(request: &NotificationRequest) -> NotificationContent {
    NotificationContent {
        title: without_bidi_formatting(request.title.as_deref().unwrap_or("Terminal")),
        body: without_bidi_formatting(&request.body),
        // macOS plays one notification sound, the one the person chose; it has
        // no error, warning, or question sounds to pick between.
        plays_sound: !matches!(request.sound, NotificationSound::Silent),
        interruption_level: request.urgency.map(|urgency| match urgency {
            NotificationUrgency::Low => InterruptionLevel::Passive,
            NotificationUrgency::Normal => InterruptionLevel::Active,
            NotificationUrgency::Critical => InterruptionLevel::TimeSensitive,
        }),
        buttons: ButtonSet {
            labels: request
                .buttons
                .iter()
                .map(|label| without_bidi_formatting(label))
                .collect(),
            foreground: request.focus_on_activation,
        },
    }
}

/// The centre's identifier for a notification: the backend's session and
/// the desktop ID the tracking state knows it by. Showing a notification
/// again under the same identifier replaces it.
fn notification_identifier(session: &str, desktop_id: u32) -> String {
    format!("{NOTIFICATION_IDENTIFIER_PREFIX}{session}-{desktop_id}")
}

/// The session and desktop ID in an identifier this module made, or `None`
/// for any other.
fn parse_notification_identifier(identifier: &str) -> Option<(&str, u32)> {
    let (session, desktop_id) = identifier
        .strip_prefix(NOTIFICATION_IDENTIFIER_PREFIX)?
        .split_once('-')?;
    if !termeric_core::hex::is_lower(session, SESSION_BYTES * 2)
        || desktop_id.starts_with(['+', '0'])
    {
        return None;
    }
    let desktop_id = desktop_id.parse().ok()?;
    Some((session, desktop_id))
}

/// The desktop ID after `current`, never 0, which the tracking state treats
/// as no notification.
fn next_desktop_id(current: u32) -> u32 {
    current.wrapping_add(1).max(1)
}

fn category_button_identifier(position: usize) -> String {
    format!("{CATEGORY_BUTTON_PREFIX}{position}")
}

/// What a person did with a notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResponseAction {
    /// Clicked the notification itself.
    Activated,
    /// Dismissed it.
    Dismissed,
    /// Pressed a button, by 1-based position.
    Button(usize),
}

/// Classifies the centre's action identifier, given the framework's own
/// identifiers for activation and dismissal.
fn response_action(action: &str, activated: &str, dismissed: &str) -> Option<ResponseAction> {
    if action == activated {
        return Some(ResponseAction::Activated);
    }
    if action == dismissed {
        return Some(ResponseAction::Dismissed);
    }
    let position = action.strip_prefix(CATEGORY_BUTTON_PREFIX)?;
    if position.starts_with(['+', '0']) {
        return None;
    }
    position.parse().ok().map(ResponseAction::Button)
}

/// The commands a response becomes for the tracking state, in order. Any
/// response ends the notification: the delegate removes one the person
/// activated, as the freedesktop service closes it, so a close follows the
/// activation. A button maps to the action key of the notification revision
/// it was pressed on (from `button_action_key`, by 1-based position), which
/// the tracking state checks against the revision it shows now.
fn response_commands(
    desktop_id: u32,
    action: ResponseAction,
    button_action_key: impl FnOnce(usize) -> Option<String>,
) -> Vec<TrackedCommand> {
    let action_key = match action {
        ResponseAction::Dismissed => None,
        ResponseAction::Activated => Some(DEFAULT_ACTION_KEY.to_owned()),
        ResponseAction::Button(position) => button_action_key(position),
    };
    action_key
        .map(|action_key| TrackedCommand::ActionInvoked {
            desktop_id,
            action_key,
        })
        .into_iter()
        .chain([TrackedCommand::NativeClosed { desktop_id }])
        .collect()
}

/// The categories registered with the centre: one per button set, the most
/// recently shown last. Identifiers are never reused within a process, so a
/// set that is evicted and comes back cannot pick up another set's labels.
struct CategoryRegistry {
    /// Keeps this process's categories apart from an earlier run's, whose
    /// notifications may still be in Notification Center.
    prefix: String,
    next_serial: u64,
    categories: VecDeque<(ButtonSet, String)>,
}

impl CategoryRegistry {
    fn new(session: &str) -> Self {
        Self {
            prefix: format!("termeric-{session}-buttons-"),
            next_serial: 1,
            categories: VecDeque::new(),
        }
    }

    /// The category identifier for `buttons`, and whether the registered set
    /// changed, so the centre needs the new one.
    fn identifier_for(&mut self, buttons: &ButtonSet) -> (String, bool) {
        if let Some(index) = self.categories.iter().position(|(set, _)| set == buttons) {
            let entry = self
                .categories
                .remove(index)
                .expect("the position was just found");
            let identifier = entry.1.clone();
            self.categories.push_back(entry);
            return (identifier, false);
        }
        let identifier = format!("{}{}", self.prefix, self.next_serial);
        self.next_serial += 1;
        self.categories
            .push_back((buttons.clone(), identifier.clone()));
        while self.categories.len() > MAX_REGISTERED_CATEGORIES {
            self.categories.pop_front();
        }
        (identifier, true)
    }

    fn categories(&self) -> impl Iterator<Item = (&ButtonSet, &str)> {
        self.categories
            .iter()
            .map(|(set, identifier)| (set, identifier.as_str()))
    }
}

/// A transmitted icon as PNG bytes for an attachment.
fn icon_png(icon: &DecodedNotificationIcon) -> Result<Vec<u8>, String> {
    let width = u32::try_from(icon.width).map_err(|_| "icon width is not representable")?;
    let height = u32::try_from(icon.height).map_err(|_| "icon height is not representable")?;
    let image = image::RgbaImage::from_raw(width, height, icon.data.clone())
        .ok_or("transmitted notification icon has inconsistent pixels")?;
    let mut encoded = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut encoded, image::ImageFormat::Png)
        .map_err(|error| format!("could not encode transmitted notification icon: {error}"))?;
    Ok(encoded.into_inner())
}

/// Whether Termeric runs from an app bundle, which the centre requires.
fn running_from_app_bundle() -> bool {
    NSBundle::mainBundle()
        .bundleIdentifier()
        .is_some_and(|identifier| identifier.length() > 0)
}

/// The process's notification centre.
struct SharedCenter(Retained<UNUserNotificationCenter>);

// SAFETY: Apple documents the shared `UNUserNotificationCenter` as usable
// from any thread: it has no main-thread requirement, and every method used
// here either takes effect asynchronously or answers through a completion
// handler on the centre's own queue. objc2's bindings do not mark the class
// `Send` or `Sync` (they leave thread safety to the caller), so this wrapper
// says so for the one object the workers of several windows share.
unsafe impl Send for SharedCenter {}
// SAFETY: As above; `&UNUserNotificationCenter` is only used to call those
// thread-safe methods, which take `&self`.
unsafe impl Sync for SharedCenter {}

/// The centre with what Termeric keeps beside it for the whole process.
struct Center {
    center: SharedCenter,
    /// The centre holds its delegate weakly; this keeps it alive.
    _delegate: Retained<CenterDelegate>,
    categories: Mutex<CategoryRegistry>,
    authorization_requested: AtomicBool,
}

static CENTER: OnceLock<Center> = OnceLock::new();

/// Each backend's session and the worker its notifications' responses go to.
/// One entry per window.
static ROUTES: Mutex<Vec<(String, SyncSender<TrackedCommand>)>> = Mutex::new(Vec::new());

impl Center {
    /// Takes the process's centre and sets its delegate. Must only run inside
    /// an app bundle.
    fn new(session: &str) -> Self {
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let delegate = CenterDelegate::new();
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        Self {
            center: SharedCenter(center),
            _delegate: delegate,
            categories: Mutex::new(CategoryRegistry::new(session)),
            authorization_requested: AtomicBool::new(false),
        }
    }

    fn get(&self) -> &UNUserNotificationCenter {
        &self.center.0
    }

    /// Asks the person, the first time in this process, to allow alerts and
    /// sounds. Once they have answered, the centre answers at once; while
    /// its prompt waits for them, notifications go on without waiting.
    fn request_authorization_once(&self) -> Result<(), String> {
        if self.authorization_requested.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let (answer, answered) = mpsc::sync_channel(1);
        let completion = RcBlock::new(move |granted: Bool, error: *mut NSError| {
            // SAFETY: The centre passes either null or a valid error that
            // outlives this call.
            let error = unsafe { error.as_ref() }.map(describe_error);
            let _ = answer.try_send(match error {
                Some(error) => Err(error),
                None if granted.as_bool() => Ok(()),
                None => Err(NOT_ALLOWED.to_owned()),
            });
        });
        self.get()
            .requestAuthorizationWithOptions_completionHandler(
                UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
                &completion,
            );
        match answered.recv_timeout(CENTER_REPLY_TIMEOUT) {
            Ok(answer) => answer,
            Err(_) => {
                log::debug!("Notification permission is waiting for the person to answer");
                Ok(())
            }
        }
    }

    /// The category for `buttons`, registered with the centre if it is new.
    fn category_for(&self, buttons: &ButtonSet) -> Result<String, String> {
        let mut registry = self
            .categories
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (identifier, changed) = registry.identifier_for(buttons);
        if changed {
            self.register_categories(&registry)?;
        }
        Ok(identifier)
    }

    /// Replaces the centre's categories with the registry's, and waits until
    /// the centre reports a set, so a notification added next finds its own.
    fn register_categories(&self, registry: &CategoryRegistry) -> Result<(), String> {
        let categories = registry
            .categories()
            .map(|(buttons, identifier)| native_category(identifier, buttons))
            .collect::<Vec<_>>();
        self.get()
            .setNotificationCategories(&NSSet::from_retained_slice(&categories));
        let (reply, replied) = mpsc::sync_channel(1);
        let completion = RcBlock::new(move |_: NonNull<NSSet<UNNotificationCategory>>| {
            let _ = reply.try_send(());
        });
        self.get()
            .getNotificationCategoriesWithCompletionHandler(&completion);
        replied.recv_timeout(CENTER_REPLY_TIMEOUT).map_err(|_| {
            "the notification centre did not register the notification's buttons".into()
        })
    }

    /// Adds a notification request, replacing any with its identifier, and
    /// returns how the centre took it.
    fn add(&self, request: &UNNotificationRequest) -> Result<(), String> {
        let (reply, replied) = mpsc::sync_channel(1);
        let completion = RcBlock::new(move |error: *mut NSError| {
            // SAFETY: The centre passes either null or a valid error that
            // outlives this call.
            let _ = reply.try_send(unsafe { error.as_ref() }.map(describe_error));
        });
        self.get()
            .addNotificationRequest_withCompletionHandler(request, Some(&*completion));
        match replied.recv_timeout(CENTER_REPLY_TIMEOUT) {
            Ok(None) => Ok(()),
            Ok(Some(error)) => Err(error),
            Err(_) => Err("the notification centre did not answer".into()),
        }
    }

    fn remove(&self, identifier: &str) {
        let identifiers = NSArray::from_retained_slice(&[NSString::from_str(identifier)]);
        self.get()
            .removeDeliveredNotificationsWithIdentifiers(&identifiers);
        self.get()
            .removePendingNotificationRequestsWithIdentifiers(&identifiers);
    }
}

/// A centre error in words, naming the setting when notifications are off.
fn describe_error(error: &NSError) -> String {
    // SAFETY: The framework defines this constant; reading it has no other
    // requirement.
    let domain = unsafe { UNErrorDomain };
    if domain.is_some_and(|domain| *error.domain() == *domain)
        && error.code() == UNErrorCode::NotificationsNotAllowed.0
    {
        return NOT_ALLOWED.to_owned();
    }
    error.localizedDescription().to_string()
}

fn native_category(identifier: &str, buttons: &ButtonSet) -> Retained<UNNotificationCategory> {
    let options = if buttons.foreground {
        UNNotificationActionOptions::Foreground
    } else {
        UNNotificationActionOptions::empty()
    };
    let actions = buttons
        .labels
        .iter()
        .enumerate()
        .map(|(index, label)| {
            UNNotificationAction::actionWithIdentifier_title_options(
                &NSString::from_str(&category_button_identifier(index + 1)),
                &NSString::from_str(label),
                options,
            )
        })
        .collect::<Vec<_>>();
    // Every category reports dismissal, so a close the person makes reaches
    // the program that asked to hear about it.
    UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
        &NSString::from_str(identifier),
        &NSArray::from_retained_slice(&actions),
        &NSArray::new(),
        UNNotificationCategoryOptions::CustomDismissAction,
    )
}

/// The notification's own button action keys, from its `userInfo`.
fn user_info(button_action_keys: &[String]) -> Retained<NSDictionary> {
    let keys = button_action_keys
        .iter()
        .map(|key| NSString::from_str(key))
        .collect::<Vec<_>>();
    let keys = NSArray::from_retained_slice(&keys);
    let keys: &AnyObject = &keys;
    let user_info = NSDictionary::<NSString, AnyObject>::from_slices(
        &[&*NSString::from_str(BUTTON_ACTION_KEYS_KEY)],
        &[keys],
    );
    // SAFETY: `NSDictionary`'s type parameters exist only at compile time;
    // every key and value is an object, which is all `userInfo` holds.
    unsafe { Retained::cast_unchecked(user_info) }
}

/// The action key of the button at `position` (1-based) on a delivered
/// notification, from the `userInfo` it was shown with.
fn delivered_button_action_key(content: &UNNotificationContent, position: usize) -> Option<String> {
    let key = NSString::from_str(BUTTON_ACTION_KEYS_KEY);
    let keys = content
        .userInfo()
        .objectForKey(&key)?
        .downcast::<NSArray>()
        .ok()?;
    let index = position.checked_sub(1)?;
    if index >= keys.count() {
        return None;
    }
    keys.objectAtIndex(index)
        .downcast::<NSString>()
        .ok()
        .map(|key| key.to_string())
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and this class does
    // not implement `Drop`.
    #[unsafe(super(NSObject))]
    struct CenterDelegate;

    unsafe impl NSObjectProtocol for CenterDelegate {}

    unsafe impl UNUserNotificationCenterDelegate for CenterDelegate {
        /// Shows notifications as banners while Termeric is frontmost too, as
        /// they are when it is not.
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present_notification(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion_handler: &DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            completion_handler.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive_notification_response(
            &self,
            center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion_handler: &DynBlock<dyn Fn()>,
        ) {
            route_response(center, response);
            completion_handler.call(());
        }
    }
);

impl CenterDelegate {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        // SAFETY: NSObject's `init` is its designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

/// Hands a person's response to the worker whose notification it was, and
/// removes a notification they activated, which the centre may otherwise
/// keep in Notification Center. The centre calls this on a queue of its own,
/// possibly the main thread, so it never waits: a worker whose queue is full
/// loses the response, and says so in the log.
fn route_response(center: &UNUserNotificationCenter, response: &UNNotificationResponse) {
    let request = response.notification().request();
    let identifier = request.identifier().to_string();
    let Some((session, desktop_id)) = parse_notification_identifier(&identifier) else {
        return;
    };
    // SAFETY: The framework defines these constants; reading them has no
    // other requirement.
    let (activated, dismissed) = unsafe {
        (
            UNNotificationDefaultActionIdentifier.to_string(),
            UNNotificationDismissActionIdentifier.to_string(),
        )
    };
    let Some(action) = response_action(
        &response.actionIdentifier().to_string(),
        &activated,
        &dismissed,
    ) else {
        return;
    };
    if action != ResponseAction::Dismissed {
        center.removeDeliveredNotificationsWithIdentifiers(&NSArray::from_retained_slice(&[
            request.identifier(),
        ]));
    }
    let content = request.content();
    let commands = response_commands(desktop_id, action, |position| {
        delivered_button_action_key(&content, position)
    });
    let worker = ROUTES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find(|(candidate, _)| candidate == session)
        .map(|(_, worker)| worker.clone());
    let Some(worker) = worker else {
        return;
    };
    for command in commands {
        if let Err(TrySendError::Full(_)) = worker.try_send(command) {
            log::warn!("A notification response was lost: the notification worker is busy");
            return;
        }
    }
}

/// One window's notifications in the process's notification centre.
pub(super) struct UserNotificationsBackend {
    session: String,
    last_desktop_id: u32,
    /// The worker's command queue, once a tracked notification has been
    /// shown: responses route to it, and closes are confirmed through it.
    commands: Option<SyncSender<TrackedCommand>>,
    last_warning: Option<String>,
}

impl UserNotificationsBackend {
    /// A backend for one window. Fails outside Termeric.app, before anything
    /// touches the centre.
    pub(super) fn new() -> Result<Self, String> {
        if !running_from_app_bundle() {
            return Err(NEEDS_APP_BUNDLE.into());
        }
        let session = termeric_core::hex::random(SESSION_BYTES)
            .map_err(|error| format!("could not start a notification session: {error}"))?;
        Ok(Self {
            session,
            last_desktop_id: 0,
            commands: None,
            last_warning: None,
        })
    }

    /// The centre, set up on first use in this process, after the person
    /// has been asked once to allow notifications.
    fn center(&self) -> Result<&'static Center, String> {
        let center = CENTER.get_or_init(|| Center::new(&self.session));
        center.request_authorization_once()?;
        Ok(center)
    }

    fn native_content(
        &mut self,
        center: &Center,
        request: &NotificationRequest,
        button_action_keys: &[String],
    ) -> Result<
        (
            Retained<UNMutableNotificationContent>,
            Option<tempfile::NamedTempFile>,
        ),
        String,
    > {
        let content = notification_content(request);
        let native = UNMutableNotificationContent::new();
        native.setTitle(&NSString::from_str(&content.title));
        native.setBody(&NSString::from_str(&content.body));
        if content.plays_sound {
            native.setSound(Some(&UNNotificationSound::defaultSound()));
        }
        // Interruption levels came with macOS 12; earlier systems would raise
        // an exception for the unknown selector.
        if let Some(level) = content.interruption_level
            && available!(macos = 12.0)
        {
            native.setInterruptionLevel(match level {
                InterruptionLevel::Passive => UNNotificationInterruptionLevel::Passive,
                InterruptionLevel::Active => UNNotificationInterruptionLevel::Active,
                InterruptionLevel::TimeSensitive => UNNotificationInterruptionLevel::TimeSensitive,
            });
        }
        native.setCategoryIdentifier(&NSString::from_str(&center.category_for(&content.buttons)?));
        if !button_action_keys.is_empty() {
            // SAFETY: `user_info` holds only property-list objects, strings in
            // an array, as `userInfo` requires.
            unsafe { native.setUserInfo(&user_info(button_action_keys)) };
        }
        let (icon, warning) = resolve_transmitted_icon(
            request.icon_cache_id.as_deref(),
            request.icon_data.as_deref(),
        );
        self.last_warning = warning;
        let icon_file = match icon.map(|icon| icon_attachment(&icon)).transpose() {
            Ok(Some((attachment, file))) => {
                native.setAttachments(&NSArray::from_retained_slice(&[attachment]));
                Some(file)
            }
            Ok(None) => None,
            Err(error) => {
                self.last_warning.get_or_insert(error);
                None
            }
        };
        Ok((native, icon_file))
    }
}

/// An attachment showing `icon`, and the private temporary file it reads.
/// The centre moves the file into its own store when the notification is
/// added; the caller removes whatever is left after that.
fn icon_attachment(
    icon: &DecodedNotificationIcon,
) -> Result<(Retained<UNNotificationAttachment>, tempfile::NamedTempFile), String> {
    let png = icon_png(icon)?;
    // Created 0600 in the per-user temporary directory.
    let mut file = tempfile::Builder::new()
        .prefix("termeric-notification-icon-")
        .suffix(".png")
        .tempfile()
        .map_err(|error| format!("could not stage the notification icon: {error}"))?;
    file.write_all(&png)
        .and_then(|()| file.flush())
        .map_err(|error| format!("could not stage the notification icon: {error}"))?;
    let path = file
        .path()
        .to_str()
        .ok_or("the notification icon's temporary path is not UTF-8")?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    // SAFETY: No options are passed, so there is no option of the wrong type.
    let attachment = unsafe {
        UNNotificationAttachment::attachmentWithIdentifier_URL_options_error(
            &NSString::from_str(ICON_ATTACHMENT_IDENTIFIER),
            &url,
            None,
        )
    }
    .map_err(|error| {
        format!(
            "could not attach the notification icon: {}",
            error.localizedDescription()
        )
    })?;
    Ok((attachment, file))
}

impl TrackedNotificationBackend for UserNotificationsBackend {
    fn prepare(&mut self, commands: SyncSender<TrackedCommand>) -> Result<(), String> {
        if self.commands.is_none() {
            ROUTES
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((self.session.clone(), commands.clone()));
            self.commands = Some(commands);
        }
        Ok(())
    }

    fn show(
        &mut self,
        request: &NotificationRequest,
        replaces_id: u32,
        button_action_keys: &[String],
    ) -> Result<u32, String> {
        let center = self.center()?;
        let (content, icon_file) = self.native_content(center, request, button_action_keys)?;
        let desktop_id = if replaces_id == 0 {
            next_desktop_id(self.last_desktop_id)
        } else {
            replaces_id
        };
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&notification_identifier(&self.session, desktop_id)),
            &content,
            None,
        );
        let added = center.add(&request);
        drop(icon_file);
        added?;
        if replaces_id == 0 {
            self.last_desktop_id = desktop_id;
        }
        Ok(desktop_id)
    }

    fn close(&mut self, desktop_id: u32) -> Result<(), String> {
        let center = self.center()?;
        center.remove(&notification_identifier(&self.session, desktop_id));
        // The centre confirms nothing, and removes delivered notifications
        // without a response. Removal cannot fail once asked, so the backend
        // confirms the close itself, in order behind the close, as the
        // freedesktop service does with its close signal. A full queue loses
        // the confirmation, and the tracking state reports the close as
        // unconfirmed when it times out.
        if let Some(commands) = &self.commands {
            let _ = commands.try_send(TrackedCommand::NativeClosed { desktop_id });
        }
        Ok(())
    }

    fn take_warning(&mut self) -> Option<String> {
        self.last_warning.take()
    }

    fn shutdown(&mut self) {
        if self.commands.take().is_some() {
            ROUTES
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .retain(|(session, _)| *session != self.session);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifications::tracked::button_action_key;

    const SESSION: &str = "0123456789abcdef";

    #[test]
    fn requests_become_plain_text_with_the_persons_sound() {
        let request = NotificationRequest {
            title: Some("Build <b>done</b>\u{202e}".into()),
            body: "Finished &\n<i>quietly</i>\u{2066}".into(),
            ..NotificationRequest::default()
        };
        assert_eq!(
            notification_content(&request),
            NotificationContent {
                title: "Build <b>done</b>".into(),
                body: "Finished &\n<i>quietly</i>".into(),
                plays_sound: true,
                interruption_level: None,
                buttons: ButtonSet {
                    labels: Vec::new(),
                    foreground: false,
                },
            }
        );
        assert_eq!(
            notification_content(&NotificationRequest::default()).title,
            "Terminal"
        );
        for sound in [
            NotificationSound::System,
            NotificationSound::Error,
            NotificationSound::Warning,
            NotificationSound::Information,
            NotificationSound::Question,
        ] {
            let request = NotificationRequest {
                sound,
                ..NotificationRequest::default()
            };
            assert!(notification_content(&request).plays_sound, "{sound:?}");
        }
        let silent = NotificationRequest {
            sound: NotificationSound::Silent,
            ..NotificationRequest::default()
        };
        assert!(!notification_content(&silent).plays_sound);
    }

    #[test]
    fn urgency_becomes_an_interruption_level() {
        for (urgency, level) in [
            (NotificationUrgency::Low, InterruptionLevel::Passive),
            (NotificationUrgency::Normal, InterruptionLevel::Active),
            (
                NotificationUrgency::Critical,
                InterruptionLevel::TimeSensitive,
            ),
        ] {
            let request = NotificationRequest {
                urgency: Some(urgency),
                ..NotificationRequest::default()
            };
            assert_eq!(
                notification_content(&request).interruption_level,
                Some(level)
            );
        }
    }

    #[test]
    fn expiry_leaves_the_content_alone() {
        // The centre has no expiry; the tracking state removes a notification
        // when its time is up.
        let shown = notification_content(&NotificationRequest::default());
        for expiry in [
            termeric_core::NotificationExpiry::Never,
            termeric_core::NotificationExpiry::AfterMillis(5_000),
        ] {
            let request = NotificationRequest {
                expiry,
                ..NotificationRequest::default()
            };
            assert_eq!(notification_content(&request), shown);
        }
    }

    #[test]
    fn buttons_keep_their_order_and_whether_they_focus() {
        let request = NotificationRequest {
            buttons: vec!["Open\u{202e}".into(), "Ignore".into()],
            focus_on_activation: true,
            ..NotificationRequest::default()
        };
        assert_eq!(
            notification_content(&request).buttons,
            ButtonSet {
                labels: vec!["Open".into(), "Ignore".into()],
                foreground: true,
            }
        );
    }

    #[test]
    fn identifiers_carry_the_session_and_desktop_id_exactly() {
        let identifier = notification_identifier(SESSION, 42);
        assert_eq!(identifier, "termeric-0123456789abcdef-42");
        assert_eq!(
            parse_notification_identifier(&identifier),
            Some((SESSION, 42))
        );
        assert_eq!(
            parse_notification_identifier(&notification_identifier(SESSION, u32::MAX)),
            Some((SESSION, u32::MAX))
        );
        for foreign in [
            "",
            "42",
            "termeric-0123456789abcdef",
            "termeric-0123456789abcdef-",
            "termeric-0123456789abcdef-0",
            "termeric-0123456789abcdef-042",
            "termeric-0123456789abcdef-+42",
            "termeric-0123456789abcdef-4294967296",
            "termeric-0123456789ABCDEF-42",
            "termeric-0123456789abcde-42",
            "termeric-0123456789abcdef-42-1",
            "other-0123456789abcdef-42",
        ] {
            assert_eq!(parse_notification_identifier(foreign), None, "{foreign}");
        }
    }

    #[test]
    fn desktop_ids_skip_zero_when_they_wrap() {
        assert_eq!(next_desktop_id(0), 1);
        assert_eq!(next_desktop_id(41), 42);
        assert_eq!(next_desktop_id(u32::MAX), 1);
    }

    #[test]
    fn responses_become_tracking_commands() {
        let classify = |action| response_action(action, "default-id", "dismiss-id");
        assert_eq!(classify("default-id"), Some(ResponseAction::Activated));
        assert_eq!(classify("dismiss-id"), Some(ResponseAction::Dismissed));
        assert_eq!(classify("button-2"), Some(ResponseAction::Button(2)));
        for unknown in ["", "button-", "button-02", "button-+2", "button-x", "other"] {
            assert_eq!(classify(unknown), None, "{unknown}");
        }

        let keys = [button_action_key(7, 1), button_action_key(7, 2)];
        let key_at = |position: usize| keys.get(position.checked_sub(1)?).cloned();
        let commands = |action| {
            response_commands(9, action, key_at)
                .into_iter()
                .map(|command| match command {
                    TrackedCommand::ActionInvoked {
                        desktop_id,
                        action_key,
                    } => format!("{desktop_id} invoked {action_key}"),
                    TrackedCommand::NativeClosed { desktop_id } => format!("{desktop_id} closed"),
                    _ => panic!("a response becomes only activations and closes"),
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            commands(ResponseAction::Activated),
            ["9 invoked default", "9 closed"]
        );
        assert_eq!(commands(ResponseAction::Dismissed), ["9 closed"]);
        assert_eq!(
            commands(ResponseAction::Button(2)),
            ["9 invoked termeric-button-7-2", "9 closed"]
        );
        // A button the notification does not have still ends it.
        assert_eq!(commands(ResponseAction::Button(3)), ["9 closed"]);
    }

    #[test]
    fn button_sets_share_categories_and_stay_bounded() {
        let set = |label: &str, foreground| ButtonSet {
            labels: vec![label.to_owned()],
            foreground,
        };
        let mut registry = CategoryRegistry::new(SESSION);
        let (first, changed) = registry.identifier_for(&set("Open", false));
        assert!(changed);
        assert_eq!(first, "termeric-0123456789abcdef-buttons-1");
        assert_eq!(
            registry.identifier_for(&set("Open", false)),
            (first.clone(), false)
        );
        let (focusing, changed) = registry.identifier_for(&set("Open", true));
        assert!(changed);
        assert_ne!(focusing, first);

        for index in 0..MAX_REGISTERED_CATEGORIES {
            registry.identifier_for(&set("Open", false));
            registry.identifier_for(&set(&format!("Label {index}"), false));
        }
        assert_eq!(registry.categories().count(), MAX_REGISTERED_CATEGORIES);
        // The set shown throughout stayed; the one shown once went first.
        assert!(
            registry
                .categories()
                .any(|(_, identifier)| identifier == first)
        );
        assert!(
            registry
                .categories()
                .all(|(_, identifier)| identifier != focusing)
        );
        // A set that comes back gets a new identifier, never an old one.
        let (returned, changed) = registry.identifier_for(&set("Open", true));
        assert!(changed);
        assert_ne!(returned, focusing);
    }

    #[test]
    fn transmitted_icons_become_pngs() {
        let icon = DecodedNotificationIcon {
            width: 2,
            height: 1,
            rowstride: 8,
            data: vec![1, 2, 3, 4, 5, 6, 7, 8],
        };
        let png = icon_png(&icon).unwrap();
        let decoded = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .unwrap()
            .into_rgba8();
        assert_eq!(decoded.dimensions(), (2, 1));
        assert_eq!(decoded.into_raw(), icon.data);

        let inconsistent = DecodedNotificationIcon {
            data: vec![0; 3],
            ..icon
        };
        assert!(icon_png(&inconsistent).is_err());
    }

    #[test]
    fn test_binaries_are_not_app_bundles() {
        // The guard that keeps every test away from the centre.
        assert!(!running_from_app_bundle());
        assert_eq!(
            UserNotificationsBackend::new().err().as_deref(),
            Some(NEEDS_APP_BUNDLE)
        );
    }
}
