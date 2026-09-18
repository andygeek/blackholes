use crate::{
    assets::AppIcon,
    model::{AppTheme, Language},
    services::providers::{AgentProvider, PlanUsageWindow, ProviderPlanUsage},
};
use chrono::{DateTime, Utc};
use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, ClickEvent, Context, FocusHandle, Pixels, Render,
    Rgba, SharedString, Window, WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind,
    WindowOptions, canvas, div, point, prelude::*, px, relative, rgb, size,
};
use gpui_component::{
    Disableable as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    v_flex,
};
use std::{cell::Cell, rc::Rc};

#[derive(Clone)]
pub(super) struct ComputerPlanUsage {
    pub provider: AgentProvider,
    pub usage: Option<ProviderPlanUsage>,
    pub refreshing: bool,
    pub failed: bool,
    pub updated_at: Option<DateTime<Utc>>,
}

impl ComputerPlanUsage {
    pub fn new(provider: AgentProvider) -> Self {
        Self {
            provider,
            usage: None,
            refreshing: false,
            failed: false,
            updated_at: None,
        }
    }

    fn windows(&self) -> &[PlanUsageWindow] {
        self.usage
            .as_ref()
            .map(|usage| usage.windows.as_slice())
            .unwrap_or_default()
    }
}

fn tr(language: Language, english: &str, spanish: &str) -> String {
    match language {
        Language::English => english.into(),
        Language::Spanish => spanish.into(),
    }
}

fn utilization(window: &PlanUsageWindow) -> Option<f64> {
    window
        .utilization
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0., 100.))
}

fn is_account_window(window: &PlanUsageWindow) -> bool {
    match window.limit_id.as_deref() {
        Some(id) => id == "codex",
        None => window.label.is_empty() || window.label.eq_ignore_ascii_case("codex"),
    }
}

// Prefer the five-hour account limit, then weekly, then any other reported
// limit. Keep its actual duration/label: primary does not necessarily mean 5h.
fn available_windows(state: &ComputerPlanUsage) -> Vec<&PlanUsageWindow> {
    let mut windows = state
        .windows()
        .iter()
        .filter(|window| utilization(window).is_some())
        .collect::<Vec<_>>();
    windows.sort_by_key(|window| {
        (
            !is_account_window(window),
            match window.minutes {
                Some(300) => 0,
                Some(10080) => 1,
                _ => 2,
            },
        )
    });
    windows
}

fn window_label(window: &PlanUsageWindow, language: Language, compact: bool) -> String {
    let duration = match window.minutes {
        Some(300) if !compact => tr(
            language,
            "Current session · 5 hours",
            "Sesión actual · 5 horas",
        ),
        Some(10080) => tr(language, "Weekly", "Semanal"),
        Some(minutes) if minutes > 0 && minutes.is_multiple_of(1440) => {
            format!("{}d", minutes / 1440)
        }
        Some(minutes) if minutes > 0 && minutes.is_multiple_of(60) => format!("{}h", minutes / 60),
        Some(minutes) if minutes > 0 => format!("{minutes}m"),
        _ => tr(language, "Usage", "Uso"),
    };
    if is_account_window(window) {
        return duration;
    }
    let name = if window.label.is_empty() {
        window.limit_id.as_deref().unwrap_or_default()
    } else {
        &window.label
    };
    if name.is_empty() {
        duration
    } else {
        format!("{name} · {duration}")
    }
}

fn reset_label(window: &PlanUsageWindow, now: DateTime<Utc>, language: Language) -> Option<String> {
    let reset = DateTime::parse_from_rfc3339(window.resets_at.as_deref()?).ok()?;
    let seconds = reset.signed_duration_since(now).num_seconds();
    if seconds <= 0 {
        return Some(tr(language, "Refresh needed", "Actualizar"));
    }
    let minutes = (seconds + 59) / 60;
    Some(if minutes >= 1440 {
        format!("{}d {}h", minutes / 1440, (minutes % 1440) / 60)
    } else if minutes >= 60 {
        format!("{}h {}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    })
}

fn provider_style(provider: AgentProvider, theme: AppTheme) -> (AppIcon, Rgba) {
    match provider {
        AgentProvider::Claude => (AppIcon::ClaudeCode, rgb(0xd97757)),
        _ => (
            AppIcon::Codex,
            if theme == AppTheme::Dark {
                rgb(0xd5dcd9)
            } else {
                rgb(0x37433c)
            },
        ),
    }
}

fn muted(theme: AppTheme) -> Rgba {
    if theme == AppTheme::Dark {
        rgb(0x969da9)
    } else {
        rgb(0x626b79)
    }
}

fn meter(value: Option<f64>, theme: AppTheme, accent: Rgba, height: f32) -> AnyElement {
    div()
        .w_full()
        .h(px(height))
        .rounded_full()
        .overflow_hidden()
        .bg(if theme == AppTheme::Dark {
            rgb(0x30343c)
        } else {
            rgb(0xd9dee6)
        })
        .when_some(value, |bar, value| {
            bar.child(
                div()
                    .w(relative(value as f32 / 100.))
                    .h_full()
                    .rounded_full()
                    .bg(accent),
            )
        })
        .into_any_element()
}

type OpenDetails = Rc<dyn Fn(AgentProvider, Bounds<Pixels>, &mut Window, &mut App)>;

fn provider_item(
    state: &ComputerPlanUsage,
    language: Language,
    theme: AppTheme,
    on_open: OpenDetails,
) -> AnyElement {
    let (icon, accent) = provider_style(state.provider, theme);
    let windows = available_windows(state);
    let window = windows.first().copied();
    let value = window.and_then(utilization);
    let summary = if let Some(value) = value {
        let reset = window
            .and_then(|window| reset_label(window, Utc::now(), language))
            .map(|reset| format!(" · {reset}"))
            .unwrap_or_default();
        format!("{value:.0}%{}{reset}", if state.failed { "*" } else { "" })
    } else if state.refreshing {
        tr(language, "Loading…", "Cargando…")
    } else {
        tr(language, "Unavailable", "No disponible")
    };
    let summary = window
        .map(|window| format!("{} · {summary}", window_label(window, language, true)))
        .unwrap_or(summary);
    let bounds = Rc::new(Cell::new(Bounds::default()));
    let painted_bounds = bounds.clone();
    let provider = state.provider;
    h_flex()
        .relative()
        .flex_none()
        .child(
            Button::new(SharedString::from(format!(
                "computer-usage-{}",
                provider.id()
            )))
            .ghost()
            .xsmall()
            .px_1()
            .text_color(muted(theme))
            .tooltip(tr(
                language,
                "View account usage limits",
                "Ver límites de uso de la cuenta",
            ))
            .child(
                h_flex()
                    .whitespace_nowrap()
                    .gap_2()
                    .child(Icon::new(icon).with_size(px(14.)).text_color(accent))
                    .child(provider.display_name())
                    .child(div().w(px(48.)).child(meter(value, theme, accent, 4.)))
                    .child(summary),
            )
            .on_click(move |_, window, cx| on_open(provider, bounds.get(), window, cx)),
        )
        .child(
            canvas(
                move |bounds, _, _| painted_bounds.set(bounds),
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
        .into_any_element()
}

fn usage_section(
    title: String,
    window: &PlanUsageWindow,
    language: Language,
    theme: AppTheme,
) -> AnyElement {
    let value = utilization(window);
    let accent = match value {
        Some(value) if value >= 90. => rgb(0xe16b65),
        Some(value) if value >= 50. => rgb(0xd9ad19),
        _ => rgb(0x8c9bec),
    };
    let used = value
        .map(|value| format!("{value:.0}% {}", tr(language, "used", "usado")))
        .unwrap_or_else(|| tr(language, "Unavailable", "No disponible"));
    let reset = reset_label(window, Utc::now(), language)
        .map(|reset| {
            if reset == tr(language, "Refresh needed", "Actualizar") {
                reset
            } else {
                format!("{} {reset}", tr(language, "Resets in", "Reinicia en"))
            }
        })
        .unwrap_or_default();
    let exact_reset = window
        .resets_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| {
            value
                .with_timezone(&chrono::Local)
                .format("%b %-d, %H:%M %Z")
                .to_string()
        });
    v_flex()
        .gap_2()
        .py_3()
        .child(div().text_size(px(13.)).child(title))
        .child(meter(value, theme, accent, 7.))
        .child(
            h_flex()
                .justify_between()
                .gap_2()
                .text_size(px(11.))
                .text_color(muted(theme))
                .child(used)
                .child(reset),
        )
        .when_some(exact_reset, |section, reset| {
            section.child(
                div()
                    .text_size(px(10.))
                    .text_color(muted(theme))
                    .child(reset),
            )
        })
        .into_any_element()
}

// A separate native panel is necessary: an in-window GPUI popover would be
// covered by the navigation/workspace WKWebViews.
pub(super) struct UsagePopover {
    pub state: ComputerPlanUsage,
    language: Language,
    theme: AppTheme,
    focus: FocusHandle,
    parent: AnyWindowHandle,
}

impl UsagePopover {
    fn close(&self, window: &mut Window, cx: &mut App) {
        window.remove_window();
        let parent = self.parent;
        cx.defer(move |cx| {
            let _ = parent.update(cx, |_, window, _| window.activate_window());
        });
    }
}

impl Render for UsagePopover {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let language = self.language;
        let theme = self.theme;
        let (icon, accent) = provider_style(self.state.provider, theme);
        let border = if theme == AppTheme::Dark {
            rgb(0x30343c)
        } else {
            rgb(0xd9dee6)
        };
        let windows = available_windows(&self.state);
        let mut sections = v_flex().gap_1();
        for (index, window) in windows.iter().enumerate() {
            if index > 0 {
                sections = sections.child(div().border_t_1().border_color(border));
            }
            sections = sections.child(usage_section(
                window_label(window, language, false),
                window,
                language,
                theme,
            ));
        }
        if windows.is_empty() {
            sections = sections.child(
                div()
                    .py_4()
                    .text_size(px(12.))
                    .text_color(muted(theme))
                    .child(if self.state.refreshing {
                        tr(language, "Loading usage…", "Cargando uso…")
                    } else {
                        tr(
                            language,
                            "No usage limits reported for this account.",
                            "Esta cuenta no reportó límites de uso.",
                        )
                    }),
            );
        }
        let status = if self.state.refreshing {
            tr(language, "Refreshing…", "Actualizando…")
        } else if self.state.failed {
            if self.state.updated_at.is_some() {
                tr(
                    language,
                    "Refresh failed. Showing the last report.",
                    "No se pudo actualizar. Se muestra el último reporte.",
                )
            } else {
                tr(
                    language,
                    "Usage unavailable. Check your local CLI account.",
                    "Uso no disponible. Revisa la cuenta de tu CLI local.",
                )
            }
        } else {
            self.state
                .updated_at
                .map(|updated| {
                    format!(
                        "{} {}",
                        tr(language, "Updated", "Actualizado"),
                        updated.with_timezone(&chrono::Local).format("%H:%M")
                    )
                })
                .unwrap_or_default()
        };
        v_flex()
            .id("usage-details-panel")
            .size_full()
            .rounded(px(10.))
            .overflow_hidden()
            .border_1()
            .border_color(border)
            .bg(if theme == AppTheme::Dark {
                rgb(0x15181e)
            } else {
                rgb(0xf8f9fc)
            })
            .text_color(if theme == AppTheme::Dark {
                rgb(0xe5e9f0)
            } else {
                rgb(0x18212f)
            })
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|view, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    view.close(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                h_flex()
                    .px_4()
                    .pt_3()
                    .gap_2()
                    .child(Icon::new(icon).with_size(px(16.)).text_color(accent))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(14.))
                            .child(self.state.provider.display_name()),
                    )
                    .child(
                        Button::new("close-usage-details")
                            .ghost()
                            .xsmall()
                            .icon(AppIcon::X)
                            .on_click(cx.listener(|view, _, window, cx| view.close(window, cx))),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .text_size(px(11.))
                    .text_color(muted(theme))
                    .child(tr(language, "Computer account", "Cuenta de la computadora")),
            )
            .child(
                div()
                    .id("usage-details-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_4()
                    .child(sections),
            )
            .child(
                div()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(border)
                    .text_size(px(10.))
                    .text_color(muted(theme))
                    .child(status),
            )
    }
}

pub(super) fn open_details(
    state: ComputerPlanUsage,
    language: Language,
    theme: AppTheme,
    trigger: Bounds<Pixels>,
    parent: &mut Window,
    cx: &mut App,
) -> anyhow::Result<WindowHandle<UsagePopover>> {
    let parent_bounds = parent.bounds();
    let display = parent.display(cx);
    let screen = display
        .as_ref()
        .map(|display| display.bounds())
        .unwrap_or(parent_bounds);
    let width = px(340.).min(screen.size.width - px(16.));
    let height = px(126. + available_windows(&state).len().max(1) as f32 * 104.)
        .min(px(560.))
        .min(screen.size.height - px(16.));
    let x = (parent_bounds.origin.x + trigger.origin.x)
        .max(screen.origin.x + px(8.))
        .min(screen.origin.x + screen.size.width - width - px(8.));
    let y = (parent_bounds.origin.y + trigger.origin.y - height - px(6.))
        .max(screen.origin.y + px(8.))
        .min(screen.origin.y + screen.size.height - height - px(8.));
    let parent_handle = parent.window_handle();
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(x, y),
                size(width, height),
            ))),
            kind: WindowKind::PopUp,
            titlebar: None,
            focus: true,
            show: true,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            display_id: display.map(|display| display.id()),
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        },
        move |window, cx| {
            cx.new(|cx| {
                let focus = cx.focus_handle();
                focus.focus(window);
                cx.observe_window_activation(window, |_, window, _| {
                    if !window.is_window_active() {
                        window.remove_window();
                    }
                })
                .detach();
                UsagePopover {
                    state,
                    language,
                    theme,
                    focus,
                    parent: parent_handle,
                }
            })
        },
    )
}

/// Resident bytes of this application process, not system-wide or peak memory.
pub(super) fn resident_memory_bytes() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::uninit();
        let size = std::mem::size_of::<libc::proc_taskinfo>() as i32;
        // SAFETY: libproc writes exactly `size` bytes to a suitably aligned buffer.
        // Only assume initialization after a complete successful response.
        let written = unsafe {
            libc::proc_pidinfo(
                std::process::id() as i32,
                libc::PROC_PIDTASKINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        if written == size {
            Some(unsafe { info.assume_init() }.pti_resident_size)
        } else {
            None
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

pub(super) fn render(
    states: &[ComputerPlanUsage; 2],
    language: Language,
    theme: AppTheme,
    terminal_count: usize,
    memory_bytes: Option<u64>,
    on_open: impl Fn(AgentProvider, Bounds<Pixels>, &mut Window, &mut App) + 'static,
    on_refresh: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let (background, border) = match theme {
        AppTheme::Dark => (rgb(0x15181e), rgb(0x252a33)),
        AppTheme::Light => (rgb(0xf0f2f6), rgb(0xd9dee8)),
    };
    let refreshing = states.iter().any(|state| state.refreshing);
    let on_open: OpenDetails = Rc::new(on_open);
    let memory = memory_bytes
        .map(|bytes| format!("{:.1} MB", bytes as f64 / 1_000_000.))
        .unwrap_or_else(|| "— MB".into());
    h_flex()
        .h(px(28.))
        .w_full()
        .flex_none()
        .px_2()
        .gap_2()
        .bg(background)
        .border_t_1()
        .border_color(border)
        .text_size(px(11.))
        .child(
            h_flex()
                .id("computer-usage-items")
                .min_w_0()
                .flex_shrink()
                .gap_2()
                .overflow_x_scroll()
                .children(
                    states
                        .iter()
                        .map(|state| provider_item(state, language, theme, on_open.clone())),
                )
                .child(
                    Button::new("refresh-computer-usage")
                        .icon(AppIcon::RefreshCw)
                        .loading_icon(Icon::new(AppIcon::RefreshCw).text_color(muted(theme)))
                        .ghost()
                        .xsmall()
                        .loading(refreshing)
                        .disabled(refreshing)
                        .flex_none()
                        .tooltip(tr(
                            language,
                            "Refresh Codex and Claude computer account usage",
                            "Actualizar el uso de las cuentas locales de Codex y Claude",
                        ))
                        .on_click(on_refresh),
                ),
        )
        .child(div().flex_1())
        .child(
            h_flex()
                .id("app-resource-usage")
                .flex_none()
                .whitespace_nowrap()
                .gap_2()
                .text_color(muted(theme))
                .child(Icon::new(AppIcon::Memory).with_size(px(14.)))
                .child(memory)
                .child("·")
                .child(Icon::new(AppIcon::SquareTerminal).with_size(px(14.)))
                .child(terminal_count.to_string()),
        )
        .into_any_element()
}
