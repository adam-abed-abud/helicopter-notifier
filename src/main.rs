use std::{
    collections::HashMap,
    env,
    error::Error,
    fs,
    io::{Read, Write},
    net::TcpListener,
    num::NonZeroU32,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, Local, SecondsFormat, Utc};
use ksni::blocking::TrayMethods;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use softbuffer::{Context, Surface};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopBuilder},
    keyboard::{Key, NamedKey},
    window::{Window, WindowAttributes, WindowId, WindowLevel},
};

#[cfg(target_os = "linux")]
use winit::platform::{wayland::EventLoopBuilderExtWayland, x11::EventLoopBuilderExtX11};

const DEFAULT_WIDTH: usize = 960;
const DEFAULT_HEIGHT: usize = 540;
const SHOW_SCENE: bool = false;
const SKY_TOP: u32 = 0xD9_F0_FF;
const SKY_BOTTOM: u32 = 0xF7_FB_FF;
const SUN: u32 = 0xFF_D7_76;
const CLOUD: u32 = 0xFF_FF_FF;
const HILL_BACK: u32 = 0xB8_D6_CB;
const HILL_FRONT: u32 = 0x8E_B7_A4;
const FIELD_TOP: u32 = 0x79_C5_77;
const FIELD_BOTTOM: u32 = 0x35_86_52;
const BODY: u32 = 0x0A_84_FF;
const BODY_LIGHT: u32 = 0x7D_C8_FF;
const BODY_DARK: u32 = 0x0A_3D_73;
const WINDOW_BLUE: u32 = 0xD7_F5_FF;
const WINDOW_DEEP: u32 = 0x49_A8_D8;
const GRAPHITE: u32 = 0x21_27_2D;
const ROTOR: u32 = 0x15_1A_20;
const SHADOW: u32 = 0x19_35_2A;
const BANNER_FILL: u32 = 0xFF_FF_FF;
const BANNER_EDGE: u32 = 0x0A_84_FF;
const BANNER_TEXT: u32 = 0x1D_1D_1F;
const GOOGLE_CALENDAR_SCOPE: &str = "https://www.googleapis.com/auth/calendar.readonly";
const GOOGLE_AUTH_URI: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";

#[derive(Debug, Clone)]
struct Cli {
    delay: Option<Duration>,
    animation_duration: Duration,
    display_backend: DisplayBackend,
    message: String,
    calendar_id: String,
    credentials_path: PathBuf,
    token_path: PathBuf,
    poll_interval: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisplayBackend {
    Auto,
    X11,
    Wayland,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let cli = parse_cli(env::args().skip(1))?;
    let _tray_handle = start_status_icon(&cli.message);

    if let Some(delay) = cli.delay {
        println!("Waiting {} second(s) before animation.", delay.as_secs());
        thread::sleep(delay);
        return animate_helicopter(cli.animation_duration, cli.display_backend, cli.message);
    }

    run_calendar_scheduler(&cli, _tray_handle.as_ref())
}

fn parse_cli(mut args: impl Iterator<Item = String>) -> Result<Cli, Box<dyn Error>> {
    let mut delay = None;
    let mut animation_duration = Duration::from_secs(14);
    let mut display_backend = DisplayBackend::Auto;
    let mut message = "HELLO FROM RUST".to_string();
    let mut calendar_id = "primary".to_string();
    let mut credentials_path = default_config_dir().join("credentials.json");
    let mut token_path = default_config_dir().join("token.json");
    let mut poll_interval = Duration::from_secs(300);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--delay" => {
                let value = args.next().ok_or("--delay requires seconds")?;
                let seconds = value.parse::<u64>()?;
                delay = Some(Duration::from_secs(seconds));
            }
            "--duration" => {
                let value = args.next().ok_or("--duration requires seconds")?;
                let seconds = value.parse::<u64>()?;
                if seconds == 0 {
                    return Err("--duration must be at least 1 second".into());
                }
                animation_duration = Duration::from_secs(seconds);
            }
            "--backend" => {
                let value = args
                    .next()
                    .ok_or("--backend requires auto, x11, or wayland")?;
                display_backend = parse_display_backend(&value)?;
            }
            "--message" | "-m" => {
                message = args.next().ok_or("--message requires text")?;
            }
            "--calendar-id" => {
                calendar_id = args.next().ok_or("--calendar-id requires a value")?;
            }
            "--credentials" => {
                credentials_path =
                    PathBuf::from(args.next().ok_or("--credentials requires a path")?);
            }
            "--token" => {
                token_path = PathBuf::from(args.next().ok_or("--token requires a path")?);
            }
            "--poll-interval" => {
                let value = args.next().ok_or("--poll-interval requires seconds")?;
                let seconds = value.parse::<u64>()?;
                if seconds == 0 {
                    return Err("--poll-interval must be at least 1 second".into());
                }
                poll_interval = Duration::from_secs(seconds);
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            unknown => return Err(format!("unknown argument: {unknown}").into()),
        }
    }

    Ok(Cli {
        delay,
        animation_duration,
        display_backend,
        message,
        calendar_id,
        credentials_path,
        token_path,
        poll_interval,
    })
}

fn default_config_dir() -> PathBuf {
    if let Some(config_home) = env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(config_home).join("helicopter")
    } else if let Some(home) = env::var_os("HOME") {
        PathBuf::from(home).join(".config").join("helicopter")
    } else {
        PathBuf::from(".")
    }
}

fn print_help() {
    println!(
        "Usage: helicopter [--calendar-id primary] [--credentials path] [--token path] [--poll-interval seconds] [--duration seconds] [--backend auto|x11|wayland]\n\n\
         Reads upcoming Google Calendar events and uses the next event title as the helicopter banner text.\n\
         --credentials defaults to ~/.config/helicopter/credentials.json.\n\
         --token defaults to ~/.config/helicopter/token.json.\n\
         --delay is useful for testing because it skips Google Calendar and uses --message.\n\
         --duration controls the flyover length; the default is 14 seconds.\n\
         --backend controls the Linux window backend; auto prefers X11 when DISPLAY is available.\n\
         --message sets the banner text; the default is HELLO FROM RUST."
    );
}

fn parse_display_backend(value: &str) -> Result<DisplayBackend, Box<dyn Error>> {
    match value {
        "auto" => Ok(DisplayBackend::Auto),
        "x11" => Ok(DisplayBackend::X11),
        "wayland" => Ok(DisplayBackend::Wayland),
        _ => Err(format!("unknown backend {value:?}; expected auto, x11, or wayland").into()),
    }
}

#[derive(Debug)]
struct HelicopterTray {
    message: String,
}

impl ksni::Tray for HelicopterTray {
    fn id(&self) -> String {
        "helicopter".into()
    }

    fn title(&self) -> String {
        "Helicopter".into()
    }

    fn icon_name(&self) -> String {
        "helicopter".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![status_icon()]
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Helicopter".into(),
            description: format!("Scheduled banner: {}", self.message),
            icon_pixmap: vec![status_icon()],
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;

        vec![
            StandardItem {
                label: format!("Message: {}", self.message),
                enabled: false,
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|_| std::process::exit(0)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

fn start_status_icon(message: &str) -> Option<ksni::blocking::Handle<HelicopterTray>> {
    let tray = HelicopterTray {
        message: normalize_banner_message(message),
    };

    match tray.spawn() {
        Ok(handle) => Some(handle),
        Err(error) => {
            eprintln!("warning: could not create GNOME status icon: {error}");
            None
        }
    }
}

fn status_icon() -> ksni::Icon {
    const SIZE: usize = 32;
    let mut data = vec![0; SIZE * SIZE * 4];

    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as i32 - 16;
            let dy = y as i32 - 16;
            let index = (y * SIZE + x) * 4;
            let inside = dx * dx + dy * dy <= 15 * 15;
            if inside {
                data[index] = 0xFF;
                data[index + 1] = 0x0A;
                data[index + 2] = 0x84;
                data[index + 3] = 0xFF;
            }
        }
    }

    draw_icon_rect(&mut data, SIZE, 6, 12, 20, 4, [0xFF, 0xF5, 0xF5, 0xF7]);
    draw_icon_rect(&mut data, SIZE, 15, 12, 2, 7, [0xFF, 0x1D, 0x1D, 0x1F]);
    draw_icon_rect(&mut data, SIZE, 9, 18, 14, 5, [0xFF, 0xF5, 0xF5, 0xF7]);
    draw_icon_rect(&mut data, SIZE, 21, 17, 4, 3, [0xFF, 0x7D, 0xC8, 0xFF]);
    draw_icon_rect(&mut data, SIZE, 6, 21, 20, 2, [0xFF, 0x1D, 0x1D, 0x1F]);

    ksni::Icon {
        width: SIZE as i32,
        height: SIZE as i32,
        data,
    }
}

fn draw_icon_rect(
    data: &mut [u8],
    size: usize,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    argb: [u8; 4],
) {
    for row in y..(y + height).min(size) {
        for col in x..(x + width).min(size) {
            let index = (row * size + col) * 4;
            data[index..index + 4].copy_from_slice(&argb);
        }
    }
}

#[derive(Debug, Clone)]
struct CalendarEvent {
    title: String,
    start: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct GoogleCredentialsFile {
    installed: Option<GoogleCredentials>,
    web: Option<GoogleCredentials>,
}

#[derive(Debug, Deserialize)]
struct GoogleCredentials {
    client_id: String,
    client_secret: String,
    #[serde(default)]
    auth_uri: Option<String>,
    #[serde(default)]
    token_uri: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredToken {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: i64,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    expires_in: i64,
}

#[derive(Debug, Deserialize)]
struct CalendarEventsResponse {
    #[serde(default)]
    items: Vec<CalendarApiEvent>,
}

#[derive(Debug, Deserialize)]
struct CalendarApiEvent {
    #[serde(default)]
    summary: Option<String>,
    start: CalendarApiEventStart,
}

#[derive(Debug, Deserialize)]
struct CalendarApiEventStart {
    #[serde(rename = "dateTime")]
    #[serde(default)]
    date_time: Option<String>,
}

fn run_calendar_scheduler(
    cli: &Cli,
    tray_handle: Option<&ksni::blocking::Handle<HelicopterTray>>,
) -> Result<(), Box<dyn Error>> {
    let client = Client::new();

    loop {
        let credentials = match read_google_credentials(&cli.credentials_path) {
            Ok(credentials) => credentials,
            Err(error) => {
                update_tray_status(tray_handle, "Google credentials missing".to_string());
                eprintln!("warning: {error}");
                thread::sleep(cli.poll_interval);
                continue;
            }
        };

        match next_calendar_event(&client, &credentials, &cli.token_path, &cli.calendar_id) {
            Ok(Some(event)) => {
                let local_start = event.start.with_timezone(&Local);
                println!(
                    "Next Google Calendar event: {} at {}",
                    event.title,
                    local_start.format("%Y-%m-%d %H:%M:%S %Z")
                );
                update_tray_status(
                    tray_handle,
                    format!(
                        "Next: {} at {}",
                        normalize_banner_message(&event.title),
                        local_start.format("%H:%M")
                    ),
                );

                if wait_until_event_or_poll(event.start, cli.poll_interval) {
                    animate_helicopter(
                        cli.animation_duration,
                        cli.display_backend,
                        event.title.clone(),
                    )?;
                }
            }
            Ok(None) => {
                update_tray_status(tray_handle, "No upcoming timed events".to_string());
                thread::sleep(cli.poll_interval);
            }
            Err(error) => {
                update_tray_status(tray_handle, format!("Calendar error: {error}"));
                eprintln!("warning: calendar lookup failed: {error}");
                thread::sleep(cli.poll_interval);
            }
        }
    }
}

fn update_tray_status(
    tray_handle: Option<&ksni::blocking::Handle<HelicopterTray>>,
    message: String,
) {
    if let Some(handle) = tray_handle {
        handle.update(|tray| tray.message = message);
    }
}

fn wait_until_event_or_poll(start: DateTime<Utc>, poll_interval: Duration) -> bool {
    let now = Utc::now();
    if start <= now {
        return true;
    }

    let wait = match (start - now).to_std() {
        Ok(wait) => wait,
        Err(_) => return true,
    };
    let sleep_for = wait.min(poll_interval);
    thread::sleep(sleep_for);
    sleep_for == wait
}

fn next_calendar_event(
    client: &Client,
    credentials: &GoogleCredentials,
    token_path: &Path,
    calendar_id: &str,
) -> Result<Option<CalendarEvent>, Box<dyn Error>> {
    let access_token = access_token(client, credentials, token_path)?;
    let time_min = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let url = format!(
        "https://www.googleapis.com/calendar/v3/calendars/{}/events",
        urlencoding::encode(calendar_id)
    );
    let response = client
        .get(url)
        .bearer_auth(access_token)
        .query(&[
            ("timeMin", time_min.as_str()),
            ("singleEvents", "true"),
            ("orderBy", "startTime"),
            ("maxResults", "10"),
        ])
        .send()?
        .error_for_status()?
        .json::<CalendarEventsResponse>()?;

    Ok(select_next_event(response.items, Utc::now()))
}

fn select_next_event(items: Vec<CalendarApiEvent>, now: DateTime<Utc>) -> Option<CalendarEvent> {
    items
        .into_iter()
        .filter_map(|event| {
            let start = event.start.date_time?;
            let start = DateTime::parse_from_rfc3339(&start)
                .ok()?
                .with_timezone(&Utc);
            if start < now {
                return None;
            }

            Some(CalendarEvent {
                title: event
                    .summary
                    .filter(|summary| !summary.trim().is_empty())
                    .unwrap_or_else(|| "Calendar Event".to_string()),
                start,
            })
        })
        .min_by_key(|event| event.start)
}

fn access_token(
    client: &Client,
    credentials: &GoogleCredentials,
    token_path: &Path,
) -> Result<String, Box<dyn Error>> {
    if let Some(mut token) = read_stored_token(token_path)? {
        if token.expires_at > unix_now() + 60 {
            return Ok(token.access_token);
        }

        if let Some(refresh_token) = token.refresh_token.clone() {
            token = refresh_access_token(client, credentials, &refresh_token)?;
            write_stored_token(token_path, &token)?;
            return Ok(token.access_token);
        }
    }

    let token = authorize_with_browser(client, credentials)?;
    write_stored_token(token_path, &token)?;
    Ok(token.access_token)
}

fn read_google_credentials(path: &Path) -> Result<GoogleCredentials, Box<dyn Error>> {
    let contents = fs::read_to_string(path).map_err(|error| {
        format!(
            "could not read Google OAuth credentials at {}: {error}. Create a Google OAuth Desktop client and save it there.",
            path.display()
        )
    })?;
    let file = serde_json::from_str::<GoogleCredentialsFile>(&contents)?;
    file.installed
        .or(file.web)
        .ok_or_else(|| "credentials.json must contain an installed or web OAuth client".into())
}

fn read_stored_token(path: &Path) -> Result<Option<StoredToken>, Box<dyn Error>> {
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&fs::read_to_string(path)?)?))
}

fn write_stored_token(path: &Path, token: &StoredToken) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(token)?)?;
    Ok(())
}

fn refresh_access_token(
    client: &Client,
    credentials: &GoogleCredentials,
    refresh_token: &str,
) -> Result<StoredToken, Box<dyn Error>> {
    let token_uri = credentials.token_uri.as_deref().unwrap_or(GOOGLE_TOKEN_URI);
    let response = client
        .post(token_uri)
        .form(&[
            ("client_id", credentials.client_id.as_str()),
            ("client_secret", credentials.client_secret.as_str()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()?
        .error_for_status()?
        .json::<TokenResponse>()?;

    Ok(StoredToken {
        access_token: response.access_token,
        refresh_token: Some(refresh_token.to_string()),
        expires_at: unix_now() + response.expires_in,
    })
}

fn authorize_with_browser(
    client: &Client,
    credentials: &GoogleCredentials,
) -> Result<StoredToken, Box<dyn Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let redirect_uri = format!(
        "http://127.0.0.1:{}/callback",
        listener.local_addr()?.port()
    );
    let state = format!("helicopter-{}", unix_now());
    let auth_uri = credentials.auth_uri.as_deref().unwrap_or(GOOGLE_AUTH_URI);
    let auth_url = format!(
        "{auth_uri}?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent&state={}",
        urlencoding::encode(&credentials.client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(GOOGLE_CALENDAR_SCOPE),
        urlencoding::encode(&state),
    );

    println!("Opening browser for Google Calendar authorization.");
    if Command::new("xdg-open").arg(&auth_url).spawn().is_err() {
        println!("Open this URL in your browser:\n{auth_url}");
    }

    let (mut stream, _) = listener.accept()?;
    let mut buffer = [0_u8; 4096];
    let bytes_read = stream.read(&mut buffer)?;
    let request = String::from_utf8_lossy(&buffer[..bytes_read]);
    let first_line = request.lines().next().ok_or("empty OAuth callback")?;
    let path = first_line
        .split_whitespace()
        .nth(1)
        .ok_or("malformed OAuth callback")?;
    let params = parse_query_params(path);
    let returned_state = params.get("state").ok_or("OAuth callback missing state")?;
    if returned_state != &state {
        return Err("OAuth callback state mismatch".into());
    }
    let code = params.get("code").ok_or("OAuth callback missing code")?;

    let body = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nHelicopter is connected to Google Calendar. You can close this tab.";
    stream.write_all(body.as_bytes()).ok();

    exchange_authorization_code(client, credentials, code, &redirect_uri)
}

fn exchange_authorization_code(
    client: &Client,
    credentials: &GoogleCredentials,
    code: &str,
    redirect_uri: &str,
) -> Result<StoredToken, Box<dyn Error>> {
    let token_uri = credentials.token_uri.as_deref().unwrap_or(GOOGLE_TOKEN_URI);
    let response = client
        .post(token_uri)
        .form(&[
            ("client_id", credentials.client_id.as_str()),
            ("client_secret", credentials.client_secret.as_str()),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
        ])
        .send()?
        .error_for_status()?
        .json::<TokenResponse>()?;

    Ok(StoredToken {
        access_token: response.access_token,
        refresh_token: response.refresh_token,
        expires_at: unix_now() + response.expires_in,
    })
}

fn parse_query_params(path: &str) -> HashMap<String, String> {
    let query = path.split_once('?').map(|(_, query)| query).unwrap_or("");
    query
        .split('&')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            Some((
                urlencoding::decode(key).ok()?.into_owned(),
                urlencoding::decode(value).ok()?.into_owned(),
            ))
        })
        .collect()
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn animate_helicopter(
    duration: Duration,
    display_backend: DisplayBackend,
    message: String,
) -> Result<(), Box<dyn Error>> {
    let event_loop = create_event_loop(display_backend)?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = AnimationApp::new(duration, message);
    event_loop.run_app(&mut app)?;
    app.result
}

fn create_event_loop(display_backend: DisplayBackend) -> Result<EventLoop<()>, Box<dyn Error>> {
    let mut builder = EventLoop::builder();
    configure_display_backend(&mut builder, display_backend);
    Ok(builder.build()?)
}

#[cfg(target_os = "linux")]
fn configure_display_backend(builder: &mut EventLoopBuilder<()>, display_backend: DisplayBackend) {
    match display_backend {
        DisplayBackend::X11 => {
            builder.with_x11();
        }
        DisplayBackend::Wayland => {
            builder.with_wayland();
        }
        DisplayBackend::Auto => {
            if env::var_os("DISPLAY").is_some() {
                builder.with_x11();
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn configure_display_backend(
    _builder: &mut EventLoopBuilder<()>,
    _display_backend: DisplayBackend,
) {
}

struct AnimationApp {
    duration: Duration,
    start: Instant,
    window: Option<Arc<Window>>,
    context: Option<Context<Arc<Window>>>,
    surface: Option<Surface<Arc<Window>, Arc<Window>>>,
    frame: Vec<u32>,
    width: usize,
    height: usize,
    message: String,
    result: Result<(), Box<dyn Error>>,
}

impl AnimationApp {
    fn new(duration: Duration, message: String) -> Self {
        Self {
            duration,
            start: Instant::now(),
            window: None,
            context: None,
            surface: None,
            frame: vec![0; DEFAULT_WIDTH * DEFAULT_HEIGHT],
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            message: normalize_banner_message(&message),
            result: Ok(()),
        }
    }

    fn create_window(&mut self, event_loop: &ActiveEventLoop) -> Result<(), Box<dyn Error>> {
        let monitor = event_loop
            .primary_monitor()
            .or_else(|| event_loop.available_monitors().next());
        let (width, height, position) = monitor
            .as_ref()
            .map(|monitor| {
                let size = monitor.size();
                (
                    size.width.max(1) as usize,
                    size.height.max(1) as usize,
                    monitor.position(),
                )
            })
            .unwrap_or((
                DEFAULT_WIDTH,
                DEFAULT_HEIGHT,
                winit::dpi::PhysicalPosition::new(0, 0),
            ));

        self.width = width;
        self.height = height;
        self.frame.resize(width * height, 0);

        let attributes = WindowAttributes::default()
            .with_title("Helicopter")
            .with_inner_size(PhysicalSize::new(width as u32, height as u32))
            .with_position(position)
            .with_resizable(false)
            .with_decorations(false)
            .with_transparent(true)
            .with_window_level(WindowLevel::AlwaysOnTop);
        let window = Arc::new(event_loop.create_window(attributes)?);
        window.set_window_level(WindowLevel::AlwaysOnTop);
        let context = Context::new(window.clone())?;
        let surface = Surface::new(&context, window.clone())?;

        self.window = Some(window);
        self.context = Some(context);
        self.surface = Some(surface);
        Ok(())
    }

    fn render(&mut self, event_loop: &ActiveEventLoop) -> Result<(), Box<dyn Error>> {
        let elapsed = self.start.elapsed();
        if elapsed >= self.duration {
            event_loop.exit();
            return Ok(());
        }

        let Some(window) = self.window.as_ref() else {
            return Ok(());
        };
        let Some(surface) = self.surface.as_mut() else {
            return Ok(());
        };

        let size = window.inner_size();
        let current_width = size.width.max(1) as usize;
        let current_height = size.height.max(1) as usize;
        if current_width != self.width || current_height != self.height {
            self.width = current_width;
            self.height = current_height;
            self.frame.resize(self.width * self.height, 0);
        }

        let width = NonZeroU32::new(size.width.max(1)).ok_or("window width is zero")?;
        let height = NonZeroU32::new(size.height.max(1)).ok_or("window height is zero")?;
        surface.resize(width, height)?;

        draw_scene(
            &mut self.frame,
            self.width,
            self.height,
            elapsed.as_secs_f32() / self.duration.as_secs_f32(),
            &self.message,
        );

        let mut surface_buffer = surface.buffer_mut()?;
        surface_buffer.copy_from_slice(&self.frame);
        surface_buffer.present()?;

        Ok(())
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: Box<dyn Error>) {
        self.result = Err(error);
        event_loop.exit();
    }
}

impl ApplicationHandler for AnimationApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            if let Err(error) = self.create_window(event_loop) {
                self.fail(event_loop, error);
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput { event, .. }
                if event.logical_key == Key::Named(NamedKey::Escape) =>
            {
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                if let Err(error) = self.render(event_loop) {
                    self.fail(event_loop, error);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }
}

struct Canvas<'a> {
    pixels: &'a mut [u32],
    width: usize,
    height: usize,
}

fn normalize_banner_message(message: &str) -> String {
    let normalized: String = message
        .chars()
        .map(|ch| {
            let upper = ch.to_ascii_uppercase();
            if glyph_rows(upper).is_some() || upper == ' ' {
                upper
            } else {
                '?'
            }
        })
        .take(28)
        .collect();

    if normalized.trim().is_empty() {
        "HELLO FROM RUST".to_string()
    } else {
        normalized
    }
}

fn draw_scene(buffer: &mut [u32], width: usize, height: usize, progress: f32, message: &str) {
    let mut canvas = Canvas {
        pixels: buffer,
        width,
        height,
    };
    let progress = progress.clamp(0.0, 1.0);
    if SHOW_SCENE {
        draw_gradient(&mut canvas, SKY_TOP, SKY_BOTTOM);
        draw_sun(&mut canvas, 785, 90, 42);
        draw_cloud(&mut canvas, cloud_x(118, width, progress, 22.0), 86, 1.1);
        draw_cloud(&mut canvas, cloud_x(492, width, progress, 34.0), 132, 0.72);
        draw_cloud(&mut canvas, cloud_x(792, width, progress, 18.0), 74, 0.92);
        draw_hills(&mut canvas);
        draw_field(&mut canvas);
    } else {
        canvas.pixels.fill(0x00_00_00_00);
    }

    let eased = smootherstep(progress);
    let x = -210.0 + eased * (width as f32 + 420.0);
    let y = height as f32 * 0.39
        + (progress * std::f32::consts::TAU * 1.15).sin() * 22.0
        + (progress * std::f32::consts::TAU * 5.0).sin() * 2.0;
    let bank = (progress * std::f32::consts::TAU).sin() * 4.0;
    let rotor_phase = progress * 68.0;

    draw_banner(
        &mut canvas,
        x.round() as i32 - 420,
        y.round() as i32 + 26,
        message,
        progress,
    );
    draw_shadow(
        &mut canvas,
        x.round() as i32 + 36,
        (height as f32 * 0.84).round() as i32,
        135,
        19,
        progress,
    );
    draw_helicopter(
        &mut canvas,
        x.round() as i32,
        y.round() as i32 + bank.round() as i32,
        rotor_phase,
    );
}

fn smootherstep(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn cloud_x(base: i32, width: usize, progress: f32, speed: f32) -> i32 {
    let wrap_width = width as f32 + 170.0;
    ((base as f32 - progress * speed + 120.0).rem_euclid(wrap_width) - 120.0).round() as i32
}

fn draw_gradient(buffer: &mut Canvas<'_>, top: u32, bottom: u32) {
    for y in 0..buffer.height {
        let t = y as f32 / (buffer.height - 1) as f32;
        let color = opaque(mix_rgb(top, bottom, t));
        let start = y * buffer.width;
        buffer.pixels[start..start + buffer.width].fill(color);
    }
}

fn draw_sun(buffer: &mut Canvas<'_>, x: i32, y: i32, radius: i32) {
    for glow in (radius..=radius + 34).rev() {
        let alpha = ((radius + 34 - glow) as f32 / 34.0 * 0.16).max(0.02);
        draw_circle_alpha(buffer, x, y, glow, SUN, alpha);
    }
    draw_circle(buffer, x, y, radius, SUN);
}

fn draw_cloud(buffer: &mut Canvas<'_>, x: i32, y: i32, scale: f32) {
    let s = |value: i32| (value as f32 * scale).round() as i32;
    draw_circle_alpha(buffer, x, y + s(18), s(24), CLOUD, 0.82);
    draw_circle_alpha(buffer, x + s(34), y, s(34), CLOUD, 0.9);
    draw_circle_alpha(buffer, x + s(74), y + s(18), s(26), CLOUD, 0.84);
    draw_circle_alpha(buffer, x + s(106), y + s(24), s(18), CLOUD, 0.72);
    draw_rect_alpha(buffer, x - s(4), y + s(19), s(116), s(26), CLOUD, 0.74);
}

fn draw_hills(buffer: &mut Canvas<'_>) {
    draw_ellipse(buffer, 188, 430, 270, 118, HILL_BACK);
    draw_ellipse(buffer, 548, 424, 340, 136, HILL_BACK);
    draw_ellipse(buffer, 820, 430, 250, 112, HILL_BACK);
    draw_rect(buffer, 0, 410, buffer.width as i32, 130, HILL_BACK);

    draw_ellipse(buffer, 130, 462, 245, 92, HILL_FRONT);
    draw_ellipse(buffer, 504, 468, 310, 106, HILL_FRONT);
    draw_ellipse(buffer, 875, 462, 240, 88, HILL_FRONT);
    draw_rect(buffer, 0, 454, buffer.width as i32, 86, HILL_FRONT);
}

fn draw_field(buffer: &mut Canvas<'_>) {
    for y in 418..buffer.height {
        let t = (y - 418) as f32 / (buffer.height - 418) as f32;
        draw_rect(
            buffer,
            0,
            y as i32,
            buffer.width as i32,
            1,
            mix_rgb(FIELD_TOP, FIELD_BOTTOM, t),
        );
    }

    for x in (0..buffer.width as i32).step_by(54) {
        draw_line_alpha(buffer, x, 540, x + 190, 418, 0xFF_FF_FF, 0.08);
    }
}

fn draw_banner(buffer: &mut Canvas<'_>, x: i32, y: i32, message: &str, progress: f32) {
    let scale = 4;
    let padding_x = 20;
    let padding_y = 12;
    let text_width = text_width(message, scale);
    let width = (text_width + padding_x * 2).max(196);
    let height = 7 * scale + padding_y * 2;
    let edge_wave = (progress * std::f32::consts::TAU * 2.0).sin().round() as i32;

    draw_thick_line(
        buffer,
        x + width,
        y + height / 2,
        x + 180,
        y - 14,
        2,
        GRAPHITE,
    );
    draw_thick_line(
        buffer,
        x + width - 2,
        y + height / 2 + 2,
        x + 180,
        y - 12,
        1,
        0xFF_FF_FF,
    );

    draw_rect_alpha(buffer, x + 5, y + 6, width, height, SHADOW, 0.14);
    draw_rect_alpha(buffer, x, y, width, height, BANNER_FILL, 0.96);
    draw_rect_alpha(buffer, x + 8, y + 7, width - 16, 7, 0xFF_FF_FF, 0.45);
    draw_rect_alpha(buffer, x, y, width, 3, BANNER_EDGE, 0.95);
    draw_rect_alpha(buffer, x, y + height - 3, width, 3, BANNER_EDGE, 0.95);
    draw_rect_alpha(buffer, x, y, 3, height, BANNER_EDGE, 0.95);
    draw_rect_alpha(
        buffer,
        x + width - 3,
        y + edge_wave,
        3,
        height,
        BANNER_EDGE,
        0.95,
    );

    draw_line(
        buffer,
        x + width - 22,
        y,
        x + width,
        y + height / 2 + edge_wave,
        BANNER_EDGE,
    );
    draw_line(
        buffer,
        x + width,
        y + height / 2 + edge_wave,
        x + width - 22,
        y + height,
        BANNER_EDGE,
    );

    draw_text(
        buffer,
        x + padding_x,
        y + padding_y,
        message,
        scale,
        BANNER_TEXT,
    );
}

fn text_width(text: &str, scale: i32) -> i32 {
    let chars = text.chars().count() as i32;
    if chars == 0 {
        0
    } else {
        chars * 5 * scale + (chars - 1) * scale
    }
}

fn draw_text(buffer: &mut Canvas<'_>, x: i32, y: i32, text: &str, scale: i32, color: u32) {
    let mut cursor = x;
    for ch in text.chars() {
        draw_glyph(buffer, cursor, y, ch, scale, color);
        cursor += 6 * scale;
    }
}

fn draw_glyph(buffer: &mut Canvas<'_>, x: i32, y: i32, ch: char, scale: i32, color: u32) {
    let Some(rows) = glyph_rows(ch) else {
        return;
    };

    for (row, bits) in rows.iter().enumerate() {
        for col in 0..5 {
            if bits & (1 << (4 - col)) != 0 {
                draw_rect(
                    buffer,
                    x + col * scale,
                    y + row as i32 * scale,
                    scale,
                    scale,
                    color,
                );
            }
        }
    }
}

fn glyph_rows(ch: char) -> Option<[u8; 7]> {
    let rows = match ch {
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01111, 0b10000, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110,
        ],
        '6' => [
            0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110,
        ],
        '!' => [
            0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100,
        ],
        '?' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b00000, 0b00100,
        ],
        '-' => [
            0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000,
        ],
        '.' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00100, 0b00100,
        ],
        ',' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00100, 0b00100, 0b01000,
        ],
        ':' => [
            0b00000, 0b00100, 0b00100, 0b00000, 0b00100, 0b00100, 0b00000,
        ],
        '\'' => [
            0b00100, 0b00100, 0b01000, 0b00000, 0b00000, 0b00000, 0b00000,
        ],
        ' ' => [0b00000; 7],
        _ => return None,
    };

    Some(rows)
}

fn draw_shadow(buffer: &mut Canvas<'_>, x: i32, y: i32, rx: i32, ry: i32, progress: f32) {
    let edge_fade = (1.0 - (progress - 0.5).abs() * 1.45).clamp(0.2, 0.75);
    for i in (0..5).rev() {
        let inset = i * 7;
        draw_ellipse_alpha(
            buffer,
            x,
            y,
            rx - inset,
            ry - inset / 4,
            SHADOW,
            edge_fade * (0.035 + i as f32 * 0.018),
        );
    }
}

fn draw_helicopter(buffer: &mut Canvas<'_>, x: i32, y: i32, rotor_phase: f32) {
    draw_tail(buffer, x, y);
    draw_skids(buffer, x, y);
    draw_rotor(buffer, x + 72, y - 62, rotor_phase);
    draw_body(buffer, x, y);
    draw_tail_rotor(buffer, x - 126, y - 34, rotor_phase);
}

fn draw_tail(buffer: &mut Canvas<'_>, x: i32, y: i32) {
    draw_thick_line(buffer, x - 80, y - 7, x + 15, y + 2, 13, BODY_DARK);
    draw_thick_line(buffer, x - 74, y - 12, x + 12, y - 2, 5, BODY_LIGHT);
    draw_thick_line(buffer, x - 82, y - 6, x - 135, y - 36, 7, BODY_DARK);
    draw_rect(buffer, x - 156, y - 47, 33, 22, BODY);
    draw_rect(buffer, x - 150, y - 43, 25, 7, BODY_LIGHT);
}

fn draw_rotor(buffer: &mut Canvas<'_>, x: i32, y: i32, phase: f32) {
    draw_ellipse_alpha(buffer, x, y, 142, 15, 0xFF_FF_FF, 0.2);
    draw_ellipse_alpha(buffer, x, y, 124, 10, ROTOR, 0.1);

    let angle = phase * std::f32::consts::TAU;
    let dx = (angle.cos() * 146.0).round() as i32;
    let dy = (angle.sin() * 17.0).round() as i32;
    draw_thick_line(buffer, x - dx, y - dy, x + dx, y + dy, 4, ROTOR);
    draw_thick_line(
        buffer,
        x - dy * 7,
        y + dx / 9,
        x + dy * 7,
        y - dx / 9,
        3,
        GRAPHITE,
    );
    draw_circle(buffer, x, y, 8, GRAPHITE);
    draw_circle(buffer, x, y, 4, BODY_LIGHT);
    draw_thick_line(buffer, x, y + 5, x, y + 39, 6, GRAPHITE);
}

fn draw_body(buffer: &mut Canvas<'_>, x: i32, y: i32) {
    draw_ellipse(buffer, x + 54, y, 86, 43, BODY_DARK);
    draw_ellipse(buffer, x + 56, y - 5, 82, 40, BODY);
    draw_ellipse_alpha(buffer, x + 36, y - 18, 60, 16, BODY_LIGHT, 0.55);
    draw_ellipse_alpha(buffer, x + 60, y + 23, 78, 13, 0x73_12_17, 0.3);

    draw_ellipse(buffer, x + 96, y - 10, 33, 29, WINDOW_DEEP);
    draw_ellipse(buffer, x + 98, y - 14, 26, 22, WINDOW_BLUE);
    draw_ellipse_alpha(buffer, x + 88, y - 23, 10, 5, 0xFF_FF_FF, 0.72);

    draw_rect(buffer, x + 4, y - 9, 54, 22, GRAPHITE);
    draw_rect_alpha(buffer, x + 8, y - 6, 46, 7, 0x66_73_80, 0.7);
    draw_rect(buffer, x - 6, y + 10, 110, 5, BODY_LIGHT);
}

fn draw_skids(buffer: &mut Canvas<'_>, x: i32, y: i32) {
    draw_thick_line(buffer, x + 10, y + 38, x + 2, y + 70, 4, GRAPHITE);
    draw_thick_line(buffer, x + 94, y + 36, x + 110, y + 70, 4, GRAPHITE);
    draw_thick_line(buffer, x - 30, y + 72, x + 145, y + 72, 5, GRAPHITE);
    draw_thick_line(buffer, x - 21, y + 68, x + 134, y + 68, 2, 0x6A_70_76);
}

fn draw_tail_rotor(buffer: &mut Canvas<'_>, x: i32, y: i32, phase: f32) {
    let spin = phase * std::f32::consts::TAU * 1.7;
    let dx = (spin.cos() * 22.0).round() as i32;
    let dy = (spin.sin() * 22.0).round() as i32;
    draw_circle_alpha(buffer, x, y, 25, 0xFF_FF_FF, 0.18);
    draw_thick_line(buffer, x - dx, y - dy, x + dx, y + dy, 3, GRAPHITE);
    draw_thick_line(buffer, x - dy, y + dx, x + dy, y - dx, 3, GRAPHITE);
    draw_circle(buffer, x, y, 5, ROTOR);
}

fn draw_rect(buffer: &mut Canvas<'_>, x: i32, y: i32, width: i32, height: i32, color: u32) {
    if width <= 0 || height <= 0 {
        return;
    }

    let x0 = x.clamp(0, buffer.width as i32) as usize;
    let y0 = y.clamp(0, buffer.height as i32) as usize;
    let x1 = (x + width).clamp(0, buffer.width as i32) as usize;
    let y1 = (y + height).clamp(0, buffer.height as i32) as usize;

    if x0 >= x1 || y0 >= y1 {
        return;
    }

    for row in y0..y1 {
        let start = row * buffer.width + x0;
        let end = row * buffer.width + x1;
        buffer.pixels[start..end].fill(opaque(color));
    }
}

fn draw_rect_alpha(
    buffer: &mut Canvas<'_>,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    color: u32,
    alpha: f32,
) {
    if width <= 0 || height <= 0 {
        return;
    }

    let x0 = x.clamp(0, buffer.width as i32) as usize;
    let y0 = y.clamp(0, buffer.height as i32) as usize;
    let x1 = (x + width).clamp(0, buffer.width as i32) as usize;
    let y1 = (y + height).clamp(0, buffer.height as i32) as usize;

    if x0 >= x1 || y0 >= y1 {
        return;
    }

    for row in y0..y1 {
        for col in x0..x1 {
            blend_pixel(buffer, col as i32, row as i32, color, alpha);
        }
    }
}

fn draw_circle(buffer: &mut Canvas<'_>, cx: i32, cy: i32, radius: i32, color: u32) {
    let r2 = radius * radius;
    for y in (cy - radius)..=(cy + radius) {
        for x in (cx - radius)..=(cx + radius) {
            let dx = x - cx;
            let dy = y - cy;
            if dx * dx + dy * dy <= r2 {
                put_pixel(buffer, x, y, color);
            }
        }
    }
}

fn draw_circle_alpha(
    buffer: &mut Canvas<'_>,
    cx: i32,
    cy: i32,
    radius: i32,
    color: u32,
    alpha: f32,
) {
    let r2 = radius * radius;
    for y in (cy - radius)..=(cy + radius) {
        for x in (cx - radius)..=(cx + radius) {
            let dx = x - cx;
            let dy = y - cy;
            if dx * dx + dy * dy <= r2 {
                blend_pixel(buffer, x, y, color, alpha);
            }
        }
    }
}

fn draw_ellipse(buffer: &mut Canvas<'_>, cx: i32, cy: i32, rx: i32, ry: i32, color: u32) {
    let rx2 = i64::from(rx) * i64::from(rx);
    let ry2 = i64::from(ry) * i64::from(ry);
    let limit = rx2 * ry2;

    for y in (cy - ry)..=(cy + ry) {
        for x in (cx - rx)..=(cx + rx) {
            let dx = i64::from(x - cx);
            let dy = i64::from(y - cy);
            if dx * dx * ry2 + dy * dy * rx2 <= limit {
                put_pixel(buffer, x, y, color);
            }
        }
    }
}

fn draw_ellipse_alpha(
    buffer: &mut Canvas<'_>,
    cx: i32,
    cy: i32,
    rx: i32,
    ry: i32,
    color: u32,
    alpha: f32,
) {
    let rx2 = i64::from(rx) * i64::from(rx);
    let ry2 = i64::from(ry) * i64::from(ry);
    let limit = rx2 * ry2;

    for y in (cy - ry)..=(cy + ry) {
        for x in (cx - rx)..=(cx + rx) {
            let dx = i64::from(x - cx);
            let dy = i64::from(y - cy);
            if dx * dx * ry2 + dy * dy * rx2 <= limit {
                blend_pixel(buffer, x, y, color, alpha);
            }
        }
    }
}

fn draw_line(buffer: &mut Canvas<'_>, mut x0: i32, mut y0: i32, x1: i32, y1: i32, color: u32) {
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;

    loop {
        put_pixel(buffer, x0, y0, color);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let twice_err = 2 * err;
        if twice_err >= dy {
            err += dy;
            x0 += sx;
        }
        if twice_err <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

fn draw_line_alpha(
    buffer: &mut Canvas<'_>,
    mut x0: i32,
    mut y0: i32,
    x1: i32,
    y1: i32,
    color: u32,
    alpha: f32,
) {
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;

    loop {
        blend_pixel(buffer, x0, y0, color, alpha);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let twice_err = 2 * err;
        if twice_err >= dy {
            err += dy;
            x0 += sx;
        }
        if twice_err <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

fn draw_thick_line(
    buffer: &mut Canvas<'_>,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    thickness: i32,
    color: u32,
) {
    let radius = (thickness / 2).max(1);
    for offset in -radius..=radius {
        draw_line(buffer, x0, y0 + offset, x1, y1 + offset, color);
        draw_line(buffer, x0 + offset, y0, x1 + offset, y1, color);
    }
    draw_circle(buffer, x0, y0, radius, color);
    draw_circle(buffer, x1, y1, radius, color);
}

fn put_pixel(buffer: &mut Canvas<'_>, x: i32, y: i32, color: u32) {
    if x >= 0 && x < buffer.width as i32 && y >= 0 && y < buffer.height as i32 {
        buffer.pixels[y as usize * buffer.width + x as usize] = opaque(color);
    }
}

fn blend_pixel(buffer: &mut Canvas<'_>, x: i32, y: i32, color: u32, alpha: f32) {
    if x >= 0 && x < buffer.width as i32 && y >= 0 && y < buffer.height as i32 {
        let index = y as usize * buffer.width + x as usize;
        buffer.pixels[index] = blend_over(buffer.pixels[index], color, alpha);
    }
}

fn opaque(color: u32) -> u32 {
    0xFF_00_00_00 | (color & 0x00_FF_FF_FF)
}

fn blend_over(dst: u32, src_rgb: u32, src_alpha: f32) -> u32 {
    let src_alpha = src_alpha.clamp(0.0, 1.0);
    let dst_alpha = ((dst >> 24) & 0xFF) as f32 / 255.0;
    let out_alpha = src_alpha + dst_alpha * (1.0 - src_alpha);

    if out_alpha <= f32::EPSILON {
        return 0;
    }

    let sr = ((src_rgb >> 16) & 0xFF) as f32;
    let sg = ((src_rgb >> 8) & 0xFF) as f32;
    let sb = (src_rgb & 0xFF) as f32;
    let dr = ((dst >> 16) & 0xFF) as f32;
    let dg = ((dst >> 8) & 0xFF) as f32;
    let db = (dst & 0xFF) as f32;

    let r = ((sr * src_alpha + dr * dst_alpha * (1.0 - src_alpha)) / out_alpha).round() as u32;
    let g = ((sg * src_alpha + dg * dst_alpha * (1.0 - src_alpha)) / out_alpha).round() as u32;
    let b = ((sb * src_alpha + db * dst_alpha * (1.0 - src_alpha)) / out_alpha).round() as u32;
    let a = (out_alpha * 255.0).round() as u32;

    (a << 24) | (r << 16) | (g << 8) | b
}

fn mix_rgb(from: u32, to: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let fr = ((from >> 16) & 0xFF) as f32;
    let fg = ((from >> 8) & 0xFF) as f32;
    let fb = (from & 0xFF) as f32;
    let tr = ((to >> 16) & 0xFF) as f32;
    let tg = ((to >> 8) & 0xFF) as f32;
    let tb = (to & 0xFF) as f32;

    let r = (fr + (tr - fr) * t).round() as u32;
    let g = (fg + (tg - fg) * t).round() as u32;
    let b = (fb + (tb - fb) * t).round() as u32;
    (r << 16) | (g << 8) | b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cli_defaults() {
        let cli = parse_cli(std::iter::empty()).unwrap();
        assert!(cli.delay.is_none());
        assert_eq!(cli.animation_duration, Duration::from_secs(14));
        assert_eq!(cli.display_backend, DisplayBackend::Auto);
        assert_eq!(cli.message, "HELLO FROM RUST");
        assert_eq!(cli.calendar_id, "primary");
        assert_eq!(
            cli.credentials_path.file_name().unwrap(),
            "credentials.json"
        );
        assert_eq!(cli.token_path.file_name().unwrap(), "token.json");
        assert_eq!(cli.poll_interval, Duration::from_secs(300));
    }

    #[test]
    fn parses_cli_delay_duration_backend_and_calendar() {
        let cli = parse_cli(
            [
                "--delay",
                "5",
                "--duration",
                "20",
                "--backend",
                "x11",
                "--message",
                "Launch 42!",
                "--calendar-id",
                "work@example.com",
                "--credentials",
                "google.json",
                "--token",
                "token.json",
                "--poll-interval",
                "30",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();

        assert_eq!(cli.delay, Some(Duration::from_secs(5)));
        assert_eq!(cli.animation_duration, Duration::from_secs(20));
        assert_eq!(cli.display_backend, DisplayBackend::X11);
        assert_eq!(cli.message, "Launch 42!");
        assert_eq!(cli.calendar_id, "work@example.com");
        assert_eq!(cli.credentials_path, PathBuf::from("google.json"));
        assert_eq!(cli.token_path, PathBuf::from("token.json"));
        assert_eq!(cli.poll_interval, Duration::from_secs(30));
    }

    #[test]
    fn rejects_zero_animation_duration() {
        let result = parse_cli(["--duration", "0"].into_iter().map(String::from));
        assert!(result.is_err());
    }

    #[test]
    fn rejects_unknown_display_backend() {
        let result = parse_cli(["--backend", "metal"].into_iter().map(String::from));
        assert!(result.is_err());
    }

    #[test]
    fn normalizes_banner_message() {
        assert_eq!(normalize_banner_message("hello!"), "HELLO!");
        assert_eq!(normalize_banner_message(""), "HELLO FROM RUST");
    }

    #[test]
    fn selects_next_timed_calendar_event() {
        let now = DateTime::parse_from_rfc3339("2026-05-30T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let events = vec![
            calendar_event("Later", Some("2026-05-30T12:00:00Z")),
            calendar_event("Soon", Some("2026-05-30T10:30:00Z")),
            calendar_event("All day", None),
        ];

        let selected = select_next_event(events, now).unwrap();
        assert_eq!(selected.title, "Soon");
        assert_eq!(
            selected.start,
            DateTime::parse_from_rfc3339("2026-05-30T10:30:00Z")
                .unwrap()
                .with_timezone(&Utc)
        );
    }

    #[test]
    fn ignores_past_and_empty_title_calendar_events() {
        let now = DateTime::parse_from_rfc3339("2026-05-30T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let events = vec![
            calendar_event("Past", Some("2026-05-30T09:30:00Z")),
            calendar_event("", Some("2026-05-30T10:30:00Z")),
        ];

        let selected = select_next_event(events, now).unwrap();
        assert_eq!(selected.title, "Calendar Event");
    }

    fn calendar_event(summary: &str, date_time: Option<&str>) -> CalendarApiEvent {
        CalendarApiEvent {
            summary: Some(summary.to_string()),
            start: CalendarApiEventStart {
                date_time: date_time.map(str::to_string),
            },
        }
    }
}
