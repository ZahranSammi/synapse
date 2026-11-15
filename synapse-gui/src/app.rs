use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    div, prelude::*, px, AnyElement, Context, FocusHandle, FontWeight, KeyDownEvent, SharedString,
    Window,
};

use crate::backend::{
    self, DashboardSnapshot, RemoteVersionItem, ServiceAction, ServiceLogFile, ALL_RUNTIMES,
    GLOBALS_RUNTIMES,
};
use crate::theme::{self, ThemeMode, ThemePalette};
use crate::widgets::{
    apply_keystroke_to_input, badge, pill_button, section_card, text_input_box, BadgeTone,
    ButtonVariant, InputKeyOutcome,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavTab {
    Runtimes,
    Services,
    Globals,
    Pins,
}

impl NavTab {
    const ALL: [Self; 4] = [Self::Runtimes, Self::Services, Self::Globals, Self::Pins];

    fn label(self) -> &'static str {
        match self {
            Self::Runtimes => "Runtimes",
            Self::Services => "Services",
            Self::Globals => "Globals",
            Self::Pins => "Project Pins",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Self::Runtimes => "Install, switch, and remove Node, Bun, PHP, Composer, Postgres, and Nginx versions.",
            Self::Services => "Start, stop, restart, and inspect logs for managed background services.",
            Self::Globals => "Manage tracked global packages, sync active runtimes, and migrate across versions.",
            Self::Pins => "Inspect and update project version pin files (.synapse-version, .node-version, etc.).",
        }
    }
}

#[derive(Debug, Clone)]
enum StatusMessage {
    Info(String),
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextFieldKind {
    RuntimeInstall,
    RemoteFilter,
    GlobalAdd,
    GlobalMigrateFrom,
    PinDir,
    PinVersion,
}

pub struct SynapseApp {
    tokio_rt: Arc<tokio::runtime::Runtime>,
    theme_mode: ThemeMode,
    active_tab: NavTab,
    snapshot: DashboardSnapshot,
    selected_runtime: String,
    selected_global_runtime: String,
    selected_pin_runtime: String,
    busy_label: Option<String>,
    status: Option<StatusMessage>,

    // Remote versions cache for currently fetched runtime
    remote_versions_for: Option<String>,
    remote_versions: Vec<RemoteVersionItem>,

    // Service logs panel
    logs_service: Option<String>,
    logs_files: Vec<ServiceLogFile>,

    // Editable input buffers + focus handles
    runtime_install_input: String,
    runtime_install_focus: FocusHandle,
    remote_filter_input: String,
    remote_filter_focus: FocusHandle,
    global_add_input: String,
    global_add_focus: FocusHandle,
    global_migrate_input: String,
    global_migrate_focus: FocusHandle,
    pin_dir_input: String,
    pin_dir_focus: FocusHandle,
    pin_version_input: String,
    pin_version_focus: FocusHandle,
}

impl SynapseApp {
    pub fn new(tokio_rt: Arc<tokio::runtime::Runtime>, cx: &mut Context<Self>) -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let pin_dir_str = cwd.to_string_lossy().to_string();
        let (snapshot, initial_status) = match backend::load_dashboard_snapshot(&cwd) {
            Ok(snap) => (snap, None),
            Err(err) => (
                DashboardSnapshot {
                    runtimes: Vec::new(),
                    services: Vec::new(),
                    globals: Vec::new(),
                    project_dir: pin_dir_str.clone(),
                    pins: Vec::new(),
                },
                Some(StatusMessage::Error(err)),
            ),
        };

        Self {
            tokio_rt,
            theme_mode: theme::load_theme_preference(),
            active_tab: NavTab::Runtimes,
            snapshot,
            selected_runtime: "node".to_string(),
            selected_global_runtime: "node".to_string(),
            selected_pin_runtime: "node".to_string(),
            busy_label: None,
            status: initial_status,
            remote_versions_for: None,
            remote_versions: Vec::new(),
            logs_service: None,
            logs_files: Vec::new(),
            runtime_install_input: String::new(),
            runtime_install_focus: cx.focus_handle(),
            remote_filter_input: String::new(),
            remote_filter_focus: cx.focus_handle(),
            global_add_input: String::new(),
            global_add_focus: cx.focus_handle(),
            global_migrate_input: String::new(),
            global_migrate_focus: cx.focus_handle(),
            pin_dir_input: pin_dir_str,
            pin_dir_focus: cx.focus_handle(),
            pin_version_input: String::new(),
            pin_version_focus: cx.focus_handle(),
        }
    }

    fn current_project_dir(&self) -> PathBuf {
        let trimmed = self.pin_dir_input.trim();
        if trimmed.is_empty() {
            PathBuf::from(&self.snapshot.project_dir)
        } else {
            PathBuf::from(trimmed)
        }
    }

    fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        self.theme_mode = self.theme_mode.toggle();
        theme::save_theme_preference(self.theme_mode);
        cx.notify();
    }

    fn reload_snapshot(&mut self, cx: &mut Context<Self>) {
        let project_dir = self.current_project_dir();
        match backend::load_dashboard_snapshot(&project_dir) {
            Ok(snap) => {
                self.snapshot = snap;
                self.status = Some(StatusMessage::Info("Dashboard refreshed".into()));
            }
            Err(err) => {
                self.status = Some(StatusMessage::Error(err));
            }
        }
        cx.notify();
    }

    fn run_bg_action<F>(&mut self, label: impl Into<String>, cx: &mut Context<Self>, work: F)
    where
        F: FnOnce(Arc<tokio::runtime::Runtime>, PathBuf) -> Result<String, String> + Send + 'static,
    {
        if self.busy_label.is_some() {
            return;
        }
        let label = label.into();
        let tokio_rt = Arc::clone(&self.tokio_rt);
        let project_dir = self.current_project_dir();

        self.busy_label = Some(label);
        self.status = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let dir_for_reload = project_dir.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let msg = work(tokio_rt, project_dir)?;
                    let snap = backend::load_dashboard_snapshot(&dir_for_reload)?;
                    Ok::<(String, DashboardSnapshot), String>((msg, snap))
                })
                .await;

            let _ = this.update(cx, |app, cx| {
                app.busy_label = None;
                match result {
                    Ok((msg, snap)) => {
                        app.snapshot = snap;
                        app.status = Some(StatusMessage::Info(msg));
                    }
                    Err(err) => {
                        app.status = Some(StatusMessage::Error(err));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn fetch_remote_versions(&mut self, runtime: String, cx: &mut Context<Self>) {
        if self.busy_label.is_some() {
            return;
        }
        let tokio_rt = Arc::clone(&self.tokio_rt);
        self.busy_label = Some(format!("Fetching {runtime} versions..."));
        self.status = None;
        cx.notify();

        let rt_name = runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { backend::fetch_remote_versions(&tokio_rt, &rt_name) })
                .await;

            let _ = this.update(cx, |app, cx| {
                app.busy_label = None;
                match result {
                    Ok(items) => {
                        let count = items.len();
                        app.remote_versions_for = Some(runtime.clone());
                        app.remote_versions = items;
                        app.status = Some(StatusMessage::Info(format!(
                            "Loaded {count} available {runtime} versions"
                        )));
                    }
                    Err(err) => {
                        app.status = Some(StatusMessage::Error(err));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_logs_for_service(&mut self, svc: String, cx: &mut Context<Self>) {
        if self.busy_label.is_some() {
            return;
        }
        self.busy_label = Some(format!("Reading {svc} logs..."));
        cx.notify();

        let svc_clone = svc.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { backend::load_service_logs(&svc_clone, 80) })
                .await;

            let _ = this.update(cx, |app, cx| {
                app.busy_label = None;
                match result {
                    Ok(files) => {
                        app.logs_service = Some(svc.clone());
                        app.logs_files = files;
                        app.status = Some(StatusMessage::Info(format!("Loaded {svc} logs")));
                    }
                    Err(err) => {
                        app.status = Some(StatusMessage::Error(err));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn trigger_install(&mut self, activate: bool, cx: &mut Context<Self>) {
        let runtime = self.selected_runtime.clone();
        let version = self.runtime_install_input.trim().to_string();
        if version.is_empty() {
            self.status = Some(StatusMessage::Error(
                "Enter a version or alias (e.g. 22, lts, latest)".into(),
            ));
            cx.notify();
            return;
        }
        self.run_bg_action(format!("Installing {runtime} {version}..."), cx, move |rt, _| {
            backend::install_runtime_version(&rt, &runtime, &version, activate)
        });
    }

    fn trigger_global_add(&mut self, cx: &mut Context<Self>) {
        let runtime = self.selected_global_runtime.clone();
        let raw_packages = self.global_add_input.trim().to_string();
        if raw_packages.is_empty() {
            self.status = Some(StatusMessage::Error(
                "Enter one or more package names separated by spaces".into(),
            ));
            cx.notify();
            return;
        }
        self.global_add_input.clear();
        self.run_bg_action(format!("Adding {runtime} globals..."), cx, move |_, _| {
            backend::globals_add_and_sync(&runtime, &raw_packages)
        });
    }

    fn trigger_pin(&mut self, cx: &mut Context<Self>) {
        let runtime = self.selected_pin_runtime.clone();
        let version = self.pin_version_input.trim().to_string();
        if version.is_empty() {
            self.status = Some(StatusMessage::Error(
                "Enter a version or alias to pin (e.g. lts, 22, 8.3)".into(),
            ));
            cx.notify();
            return;
        }
        self.run_bg_action(format!("Pinning {runtime} {version}..."), cx, move |rt, dir| {
            backend::pin_runtime_in_dir(&rt, &dir, &runtime, &version)
        });
    }

    fn field_value_and_handle(&self, kind: TextFieldKind) -> (&str, FocusHandle) {
        match kind {
            TextFieldKind::RuntimeInstall => {
                (&self.runtime_install_input, self.runtime_install_focus.clone())
            }
            TextFieldKind::RemoteFilter => {
                (&self.remote_filter_input, self.remote_filter_focus.clone())
            }
            TextFieldKind::GlobalAdd => (&self.global_add_input, self.global_add_focus.clone()),
            TextFieldKind::GlobalMigrateFrom => {
                (&self.global_migrate_input, self.global_migrate_focus.clone())
            }
            TextFieldKind::PinDir => (&self.pin_dir_input, self.pin_dir_focus.clone()),
            TextFieldKind::PinVersion => {
                (&self.pin_version_input, self.pin_version_focus.clone())
            }
        }
    }

    fn field_buffer_mut(&mut self, kind: TextFieldKind) -> &mut String {
        match kind {
            TextFieldKind::RuntimeInstall => &mut self.runtime_install_input,
            TextFieldKind::RemoteFilter => &mut self.remote_filter_input,
            TextFieldKind::GlobalAdd => &mut self.global_add_input,
            TextFieldKind::GlobalMigrateFrom => &mut self.global_migrate_input,
            TextFieldKind::PinDir => &mut self.pin_dir_input,
            TextFieldKind::PinVersion => &mut self.pin_version_input,
        }
    }

    fn handle_field_submit(&mut self, kind: TextFieldKind, cx: &mut Context<Self>) {
        match kind {
            TextFieldKind::RuntimeInstall => self.trigger_install(true, cx),
            TextFieldKind::GlobalAdd => self.trigger_global_add(cx),
            TextFieldKind::GlobalMigrateFrom => {
                let runtime = self.selected_global_runtime.clone();
                let from_ver = self.global_migrate_input.trim().to_string();
                if from_ver.is_empty() {
                    self.status = Some(StatusMessage::Error(
                        "Enter a source version to migrate from".into(),
                    ));
                    cx.notify();
                    return;
                }
                self.run_bg_action(format!("Migrating {runtime} globals..."), cx, move |_, _| {
                    backend::globals_migrate(&runtime, &from_ver)
                });
            }
            TextFieldKind::PinDir => self.reload_snapshot(cx),
            TextFieldKind::PinVersion => self.trigger_pin(cx),
            TextFieldKind::RemoteFilter => {}
        }
    }

    fn render_input_field(
        &self,
        id: &'static str,
        kind: TextFieldKind,
        placeholder: &'static str,
        window: &Window,
        cx: &mut Context<Self>,
        palette: &ThemePalette,
    ) -> AnyElement {
        let (value, handle) = self.field_value_and_handle(kind);
        let focus_on_click = handle.clone();

        text_input_box(
            id,
            value,
            placeholder,
            &handle,
            window,
            palette,
            move |_ev, window, _cx| {
                focus_on_click.focus(window);
            },
            cx.listener(move |this, ev: &KeyDownEvent, window, cx| {
                let key = ev.keystroke.key.as_str();
                let key_char = ev.keystroke.key_char.as_deref();
                let outcome = apply_keystroke_to_input(this.field_buffer_mut(kind), key, key_char);
                match outcome {
                    InputKeyOutcome::Changed => cx.notify(),
                    InputKeyOutcome::Submit => this.handle_field_submit(kind, cx),
                    InputKeyOutcome::Escape => {
                        window.blur();
                        cx.notify();
                    }
                    InputKeyOutcome::Ignored => {}
                }
            }),
        )
    }

    fn render_runtime_chips(
        &self,
        id_prefix: &'static str,
        runtimes: &[&'static str],
        selected: &str,
        cx: &mut Context<Self>,
        palette: &ThemePalette,
        on_select: impl Fn(&mut Self, &str) + Copy + 'static,
    ) -> AnyElement {
        let mut row = div().flex().flex_wrap().gap(px(8.0));
        for &rt in runtimes {
            let is_selected = rt == selected;
            let active_ver = self
                .snapshot
                .runtimes
                .iter()
                .find(|r| r.id == rt)
                .and_then(|r| r.active_version.clone());

            let label = match active_ver {
                Some(v) => format!("{} ({v})", backend::runtime_display_name(rt)),
                None => backend::runtime_display_name(rt).to_string(),
            };
            let rt_owned = rt.to_string();
            let chip_id: SharedString = format!("{id_prefix}-{rt}").into();

            row = row.child(
                div()
                    .id(chip_id)
                    .px(px(12.0))
                    .py(px(6.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(if is_selected {
                        palette.accent
                    } else {
                        palette.border
                    })
                    .bg(if is_selected {
                        palette.accent_soft
                    } else {
                        palette.bg_surface
                    })
                    .text_xs()
                    .font_weight(if is_selected {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .text_color(if is_selected {
                        palette.accent
                    } else {
                        palette.text_primary
                    })
                    .cursor_pointer()
                    .hover(|s| s.bg(palette.bg_hover))
                    .on_click(cx.listener(move |this, _ev, _window, cx| {
                        on_select(this, &rt_owned);
                        cx.notify();
                    }))
                    .child(label),
            );
        }
        row.into_any_element()
    }

    fn render_sidebar(&self, cx: &mut Context<Self>, palette: &ThemePalette) -> AnyElement {
        let active_runtimes_count = self
            .snapshot
            .runtimes
            .iter()
            .filter(|r| r.active_version.is_some())
            .count();
        let running_services_count = self
            .snapshot
            .services
            .iter()
            .filter(|s| s.running)
            .count();
        let pins_count = self.snapshot.pins.len();

        let mut nav_list = div().flex().flex_col().gap(px(4.0));
        for tab in NavTab::ALL {
            let is_active = self.active_tab == tab;
            let count_text: Option<String> = match tab {
                NavTab::Runtimes => Some(format!("{active_runtimes_count}/{}", ALL_RUNTIMES.len())),
                NavTab::Services => Some(format!(
                    "{running_services_count}/{}",
                    self.snapshot.services.len()
                )),
                NavTab::Globals => Some(format!("{}", GLOBALS_RUNTIMES.len())),
                NavTab::Pins => Some(format!("{pins_count}")),
            };
            let tab_id: SharedString = format!("nav-{}", tab.label()).into();

            nav_list = nav_list.child(
                div()
                    .id(tab_id)
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(12.0))
                    .py(px(9.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(if is_active {
                        palette.accent
                    } else {
                        palette.bg_sidebar
                    })
                    .bg(if is_active {
                        palette.accent_soft
                    } else {
                        palette.bg_sidebar
                    })
                    .cursor_pointer()
                    .hover(|s| s.bg(palette.bg_hover))
                    .on_click(cx.listener(move |this, _ev, _window, cx| {
                        this.active_tab = tab;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(if is_active {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::MEDIUM
                            })
                            .text_color(if is_active {
                                palette.accent
                            } else {
                                palette.text_primary
                            })
                            .child(tab.label()),
                    )
                    .when_some(count_text, |el, count| {
                        el.child(
                            div()
                                .text_xs()
                                .text_color(palette.text_muted)
                                .child(count),
                        )
                    }),
            );
        }

        let theme_button_label = match self.theme_mode {
            ThemeMode::Dark => "Theme: Dark  -> Light",
            ThemeMode::Light => "Theme: Light -> Dark",
        };

        div()
            .flex()
            .flex_col()
            .justify_between()
            .w(px(236.0))
            .h_full()
            .p(px(16.0))
            .bg(palette.bg_sidebar)
            .border_r_1()
            .border_color(palette.border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(20.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_lg()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(palette.text_primary)
                                            .child("Synapse"),
                                    )
                                    .child(badge("GUI", BadgeTone::Accent, palette)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(palette.text_muted)
                                    .child("Universal Runtime Manager"),
                            ),
                    )
                    .child(nav_list),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .pt(px(12.0))
                    .border_t_1()
                    .border_color(palette.border_subtle)
                    .child(pill_button(
                        "theme-toggle-btn",
                        theme_button_label,
                        ButtonVariant::Secondary,
                        false,
                        palette,
                        cx.listener(|this, _ev, _window, cx| this.toggle_theme(cx)),
                    ))
                    .child(pill_button(
                        "refresh-snapshot-btn",
                        "Refresh State",
                        ButtonVariant::Ghost,
                        self.busy_label.is_some(),
                        palette,
                        cx.listener(|this, _ev, _window, cx| this.reload_snapshot(cx)),
                    )),
            )
            .into_any_element()
    }

    fn render_banner(&self, cx: &mut Context<Self>, palette: &ThemePalette) -> Option<AnyElement> {
        if let Some(busy) = &self.busy_label {
            return Some(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(px(8.0))
                    .bg(palette.accent_soft)
                    .border_1()
                    .border_color(palette.accent)
                    .text_xs()
                    .text_color(palette.accent)
                    .child(busy.clone())
                    .into_any_element(),
            );
        }

        let status = self.status.as_ref()?;
        let (msg, bg, fg, border) = match status {
            StatusMessage::Info(text) => (text, palette.success_bg, palette.success, palette.success),
            StatusMessage::Error(text) => (text, palette.danger_bg, palette.danger, palette.danger),
        };

        Some(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .px(px(12.0))
                .py(px(8.0))
                .rounded(px(8.0))
                .bg(bg)
                .border_1()
                .border_color(border)
                .child(div().flex_1().text_xs().text_color(fg).child(msg.clone()))
                .child(pill_button(
                    "dismiss-status-btn",
                    "Dismiss",
                    ButtonVariant::Ghost,
                    false,
                    palette,
                    cx.listener(|this, _ev, _window, cx| {
                        this.status = None;
                        cx.notify();
                    }),
                ))
                .into_any_element(),
        )
    }

    fn render_runtimes_tab(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
        palette: &ThemePalette,
    ) -> AnyElement {
        let busy = self.busy_label.is_some();
        let selected = self
            .snapshot
            .runtimes
            .iter()
            .find(|r| r.id == self.selected_runtime);

        let rt_id = self.selected_runtime.clone();
        let display_name = selected
            .map(|r| r.display_name)
            .unwrap_or_else(|| backend::runtime_display_name(&rt_id));
        let active_ver = selected.and_then(|r| r.active_version.clone());
        let installed = selected
            .map(|r| r.installed_versions.clone())
            .unwrap_or_default();

        let mut installed_rows = div().flex().flex_col().gap(px(8.0));
        if installed.is_empty() {
            installed_rows = installed_rows.child(
                div()
                    .p(px(12.0))
                    .rounded(px(8.0))
                    .bg(palette.bg_surface_alt)
                    .text_xs()
                    .text_color(palette.text_muted)
                    .child(format!("No {display_name} versions installed yet.")),
            );
        } else {
            for ver in installed {
                let is_active = active_ver.as_deref() == Some(ver.as_str());
                let rt_for_use = rt_id.clone();
                let ver_for_use = ver.clone();
                let rt_for_del = rt_id.clone();
                let ver_for_del = ver.clone();
                let use_id: SharedString = format!("use-{rt_id}-{ver}").into();
                let del_id: SharedString = format!("del-{rt_id}-{ver}").into();

                installed_rows = installed_rows.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(px(12.0))
                        .py(px(9.0))
                        .rounded(px(8.0))
                        .bg(palette.bg_surface_alt)
                        .border_1()
                        .border_color(if is_active {
                            palette.success
                        } else {
                            palette.border_subtle
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(palette.text_primary)
                                        .child(ver.clone()),
                                )
                                .when(is_active, |el| {
                                    el.child(badge("Active", BadgeTone::Success, palette))
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(pill_button(
                                    use_id,
                                    "Use",
                                    ButtonVariant::Secondary,
                                    busy || is_active,
                                    palette,
                                    cx.listener(move |this, _ev, _window, cx| {
                                        let rt = rt_for_use.clone();
                                        let v = ver_for_use.clone();
                                        this.run_bg_action(
                                            format!("Switching {rt} to {v}..."),
                                            cx,
                                            move |_, _| backend::switch_runtime_version(&rt, &v),
                                        );
                                    }),
                                ))
                                .child(pill_button(
                                    del_id,
                                    "Uninstall",
                                    ButtonVariant::Danger,
                                    busy,
                                    palette,
                                    cx.listener(move |this, _ev, _window, cx| {
                                        let rt = rt_for_del.clone();
                                        let v = ver_for_del.clone();
                                        this.run_bg_action(
                                            format!("Removing {rt} {v}..."),
                                            cx,
                                            move |_, _| backend::uninstall_runtime_version(&rt, &v),
                                        );
                                    }),
                                )),
                        ),
                );
            }
        }

        let main_card = section_card(palette)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(palette.text_primary)
                                    .child(display_name),
                            )
                            .child(match &active_ver {
                                Some(v) => badge(format!("Active: {v}"), BadgeTone::Success, palette),
                                None => badge("Not active", BadgeTone::Neutral, palette),
                            }),
                    )
                    .child({
                        let rt_fetch = rt_id.clone();
                        pill_button(
                            "fetch-remote-versions-btn",
                            "Fetch Remote Versions",
                            ButtonVariant::Secondary,
                            busy,
                            palette,
                            cx.listener(move |this, _ev, _window, cx| {
                                this.fetch_remote_versions(rt_fetch.clone(), cx);
                            }),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(self.render_input_field(
                        "runtime-install-input",
                        TextFieldKind::RuntimeInstall,
                        "Version or alias (e.g. 22, lts, latest, 8.3)",
                        window,
                        cx,
                        palette,
                    ))
                    .child(pill_button(
                        "install-only-btn",
                        "Install",
                        ButtonVariant::Secondary,
                        busy,
                        palette,
                        cx.listener(|this, _ev, _window, cx| this.trigger_install(false, cx)),
                    ))
                    .child(pill_button(
                        "install-and-use-btn",
                        "Install & Use",
                        ButtonVariant::Primary,
                        busy,
                        palette,
                        cx.listener(|this, _ev, _window, cx| this.trigger_install(true, cx)),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(palette.text_secondary)
                            .child("INSTALLED VERSIONS"),
                    )
                    .child(installed_rows),
            );

        let mut root = div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(self.render_runtime_chips(
                "rt-chip",
                ALL_RUNTIMES,
                &self.selected_runtime,
                cx,
                palette,
                |this, rt| {
                    this.selected_runtime = rt.to_string();
                },
            ))
            .child(main_card);

        if self.remote_versions_for.as_deref() == Some(self.selected_runtime.as_str()) {
            let filter = self.remote_filter_input.trim().to_lowercase();
            let filtered: Vec<&RemoteVersionItem> = self
                .remote_versions
                .iter()
                .filter(|item| {
                    filter.is_empty()
                        || item.version.to_lowercase().contains(&filter)
                        || item
                            .lts
                            .as_ref()
                            .is_some_and(|lts| lts.to_lowercase().contains(&filter))
                })
                .take(24)
                .collect();

            let mut list = div().flex().flex_wrap().gap(px(8.0));
            for item in filtered {
                let ver = item.version.clone();
                let rt_install = rt_id.clone();
                let ver_install = ver.clone();
                let btn_id: SharedString = format!("remote-inst-{rt_id}-{ver}").into();
                let label = match &item.lts {
                    Some(lts) => format!("{ver} ({lts})"),
                    None => ver,
                };
                list = list.child(pill_button(
                    btn_id,
                    label,
                    ButtonVariant::Secondary,
                    busy,
                    palette,
                    cx.listener(move |this, _ev, _window, cx| {
                        let rt = rt_install.clone();
                        let v = ver_install.clone();
                        this.run_bg_action(format!("Installing {rt} {v}..."), cx, move |rt_handle, _| {
                            backend::install_runtime_version(&rt_handle, &rt, &v, true)
                        });
                    }),
                ));
            }

            root = root.child(
                section_card(palette)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(palette.text_primary)
                                    .child(format!("Available Remote {display_name} Versions (click to install & use)")),
                            )
                            .child(self.render_input_field(
                                "remote-filter-input",
                                TextFieldKind::RemoteFilter,
                                "Filter remote versions...",
                                window,
                                cx,
                                palette,
                            )),
                    )
                    .child(list),
            );
        }

        root.into_any_element()
    }

    fn render_services_tab(&self, cx: &mut Context<Self>, palette: &ThemePalette) -> AnyElement {
        let busy = self.busy_label.is_some();
        let mut cards = div().flex().flex_col().gap(px(12.0));

        for svc in &self.snapshot.services {
            let status_badge = if svc.running {
                let txt = svc
                    .pid
                    .map(|p| format!("Running (PID {p})"))
                    .unwrap_or_else(|| "Running".to_string());
                badge(txt, BadgeTone::Success, palette)
            } else {
                badge("Stopped", BadgeTone::Neutral, palette)
            };

            let svc_start = svc.id.to_string();
            let svc_stop = svc.id.to_string();
            let svc_restart = svc.id.to_string();
            let svc_logs = svc.id.to_string();

            cards = cards.child(
                section_card(palette)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.0))
                                    .child(
                                        div()
                                            .text_base()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(palette.text_primary)
                                            .child(svc.display_name),
                                    )
                                    .child(status_badge),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(pill_button(
                                        SharedString::from(format!("svc-start-{}", svc.id)),
                                        "Start",
                                        ButtonVariant::Primary,
                                        busy || svc.running,
                                        palette,
                                        cx.listener(move |this, _ev, _window, cx| {
                                            let s = svc_start.clone();
                                            this.run_bg_action(
                                                format!("Starting {s}..."),
                                                cx,
                                                move |_, _| {
                                                    backend::run_service_action(
                                                        &s,
                                                        ServiceAction::Start,
                                                    )
                                                },
                                            );
                                        }),
                                    ))
                                    .child(pill_button(
                                        SharedString::from(format!("svc-stop-{}", svc.id)),
                                        "Stop",
                                        ButtonVariant::Danger,
                                        busy || !svc.running,
                                        palette,
                                        cx.listener(move |this, _ev, _window, cx| {
                                            let s = svc_stop.clone();
                                            this.run_bg_action(
                                                format!("Stopping {s}..."),
                                                cx,
                                                move |_, _| {
                                                    backend::run_service_action(
                                                        &s,
                                                        ServiceAction::Stop,
                                                    )
                                                },
                                            );
                                        }),
                                    ))
                                    .child(pill_button(
                                        SharedString::from(format!("svc-restart-{}", svc.id)),
                                        "Restart",
                                        ButtonVariant::Secondary,
                                        busy,
                                        palette,
                                        cx.listener(move |this, _ev, _window, cx| {
                                            let s = svc_restart.clone();
                                            this.run_bg_action(
                                                format!("Restarting {s}..."),
                                                cx,
                                                move |_, _| {
                                                    backend::run_service_action(
                                                        &s,
                                                        ServiceAction::Restart,
                                                    )
                                                },
                                            );
                                        }),
                                    ))
                                    .child(pill_button(
                                        SharedString::from(format!("svc-logs-{}", svc.id)),
                                        "View Logs",
                                        ButtonVariant::Ghost,
                                        busy,
                                        palette,
                                        cx.listener(move |this, _ev, _window, cx| {
                                            this.load_logs_for_service(svc_logs.clone(), cx);
                                        }),
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette.text_secondary)
                            .child(svc.detail.clone()),
                    ),
            );
        }

        if let Some(log_svc) = &self.logs_service {
            let mut log_card = section_card(palette).child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(palette.text_primary)
                            .child(format!("Logs: {log_svc} (last 80 lines)")),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child({
                                let svc_reload = log_svc.clone();
                                pill_button(
                                    "reload-logs-btn",
                                    "Reload",
                                    ButtonVariant::Secondary,
                                    busy,
                                    palette,
                                    cx.listener(move |this, _ev, _window, cx| {
                                        this.load_logs_for_service(svc_reload.clone(), cx);
                                    }),
                                )
                            })
                            .child(pill_button(
                                "close-logs-btn",
                                "Close",
                                ButtonVariant::Ghost,
                                false,
                                palette,
                                cx.listener(|this, _ev, _window, cx| {
                                    this.logs_service = None;
                                    this.logs_files.clear();
                                    cx.notify();
                                }),
                            )),
                    ),
            );

            for file in &self.logs_files {
                log_card = log_card.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .p(px(12.0))
                        .rounded(px(8.0))
                        .bg(palette.bg_surface_alt)
                        .border_1()
                        .border_color(palette.border_subtle)
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(palette.text_secondary)
                                .child(format!("{} — {}", file.label, file.path)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(palette.text_primary)
                                .child(file.content.clone()),
                        ),
                );
            }

            cards = cards.child(log_card);
        }

        cards.into_any_element()
    }

    fn render_globals_tab(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
        palette: &ThemePalette,
    ) -> AnyElement {
        let busy = self.busy_label.is_some();
        let rt_id = self.selected_global_runtime.clone();
        let overview = self.snapshot.globals.iter().find(|g| g.runtime == rt_id);

        let active_ver = overview.and_then(|g| g.active_version.clone());
        let manifest_path = overview
            .map(|g| g.manifest_path.clone())
            .unwrap_or_default();
        let tracked = overview.map(|g| g.tracked.clone()).unwrap_or_default();
        let installed = overview.map(|g| g.installed.clone()).unwrap_or_default();

        let other_versions: Vec<String> = self
            .snapshot
            .runtimes
            .iter()
            .find(|r| r.id == rt_id)
            .map(|r| {
                r.installed_versions
                    .iter()
                    .filter(|v| active_ver.as_deref() != Some(v.as_str()))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        let mut tracked_list = div().flex().flex_wrap().gap(px(8.0));
        if tracked.is_empty() {
            tracked_list = tracked_list.child(
                div()
                    .text_xs()
                    .text_color(palette.text_muted)
                    .child("No tracked global packages in manifest."),
            );
        } else {
            for pkg in tracked {
                let rt_rm = rt_id.clone();
                let pkg_rm = pkg.clone();
                let rm_id: SharedString = format!("rm-global-{rt_id}-{pkg}").into();
                tracked_list = tracked_list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(10.0))
                        .py(px(5.0))
                        .rounded(px(8.0))
                        .bg(palette.bg_surface_alt)
                        .border_1()
                        .border_color(palette.border_subtle)
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(palette.text_primary)
                                .child(pkg.clone()),
                        )
                        .child(pill_button(
                            rm_id,
                            "Remove",
                            ButtonVariant::Danger,
                            busy,
                            palette,
                            cx.listener(move |this, _ev, _window, cx| {
                                let rt = rt_rm.clone();
                                let p = pkg_rm.clone();
                                this.run_bg_action(
                                    format!("Removing {p} from {rt} globals..."),
                                    cx,
                                    move |_, _| backend::globals_remove(&rt, &p),
                                );
                            }),
                        )),
                );
            }
        }

        let mut installed_list = div().flex().flex_wrap().gap(px(8.0));
        if installed.is_empty() {
            installed_list = installed_list.child(
                div()
                    .text_xs()
                    .text_color(palette.text_muted)
                    .child("No installed global packages detected in active version."),
            );
        } else {
            for pkg in installed {
                installed_list = installed_list.child(badge(pkg, BadgeTone::Neutral, palette));
            }
        }

        let rt_sync = rt_id.clone();
        let rt_migrate = rt_id.clone();
        let mut migrate_chips = div().flex().flex_wrap().gap(px(6.0));
        for ver in other_versions {
            let chip_id: SharedString = format!("mig-src-{rt_id}-{ver}").into();
            let v_fill = ver.clone();
            migrate_chips = migrate_chips.child(pill_button(
                chip_id,
                format!("From {ver}"),
                ButtonVariant::Ghost,
                busy,
                palette,
                cx.listener(move |this, _ev, _window, cx| {
                    this.global_migrate_input = v_fill.clone();
                    cx.notify();
                }),
            ));
        }

        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(self.render_runtime_chips(
                "glob-chip",
                GLOBALS_RUNTIMES,
                &self.selected_global_runtime,
                cx,
                palette,
                |this, rt| {
                    this.selected_global_runtime = rt.to_string();
                },
            ))
            .child(
                section_card(palette)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.0))
                                    .child(
                                        div()
                                            .text_base()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(palette.text_primary)
                                            .child(format!(
                                                "{} Globals",
                                                backend::runtime_display_name(&rt_id)
                                            )),
                                    )
                                    .child(match &active_ver {
                                        Some(v) => {
                                            badge(format!("Active: {v}"), BadgeTone::Success, palette)
                                        }
                                        None => badge(
                                            "No active version",
                                            BadgeTone::Warning,
                                            palette,
                                        ),
                                    }),
                            )
                            .child(pill_button(
                                "globals-sync-btn",
                                "Sync Manifest -> Active",
                                ButtonVariant::Secondary,
                                busy || active_ver.is_none(),
                                palette,
                                cx.listener(move |this, _ev, _window, cx| {
                                    let rt = rt_sync.clone();
                                    this.run_bg_action(
                                        format!("Syncing {rt} globals..."),
                                        cx,
                                        move |_, _| backend::globals_sync(&rt),
                                    );
                                }),
                            )),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette.text_muted)
                            .child(format!("Manifest: {manifest_path}")),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(self.render_input_field(
                                "global-add-input",
                                TextFieldKind::GlobalAdd,
                                "Packages to add & sync (space-separated, e.g. typescript eslint)",
                                window,
                                cx,
                                palette,
                            ))
                            .child(pill_button(
                                "global-add-btn",
                                "Add & Sync",
                                ButtonVariant::Primary,
                                busy,
                                palette,
                                cx.listener(|this, _ev, _window, cx| this.trigger_global_add(cx)),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(palette.text_secondary)
                                    .child("TRACKED MANIFEST PACKAGES"),
                            )
                            .child(tracked_list),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(palette.text_secondary)
                                    .child("INSTALLED IN ACTIVE VERSION"),
                            )
                            .child(installed_list),
                    ),
            )
            .child(
                section_card(palette)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(palette.text_primary)
                            .child("Migrate Globals From Previous Version"),
                    )
                    .child(migrate_chips)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(self.render_input_field(
                                "global-migrate-input",
                                TextFieldKind::GlobalMigrateFrom,
                                "Source version (e.g. 20.18.0)",
                                window,
                                cx,
                                palette,
                            ))
                            .child(pill_button(
                                "global-migrate-btn",
                                "Migrate -> Active",
                                ButtonVariant::Secondary,
                                busy || active_ver.is_none(),
                                palette,
                                cx.listener(move |this, _ev, _window, cx| {
                                    let rt = rt_migrate.clone();
                                    let from_ver = this.global_migrate_input.trim().to_string();
                                    if from_ver.is_empty() {
                                        this.status = Some(StatusMessage::Error(
                                            "Enter a source version to migrate from".into(),
                                        ));
                                        cx.notify();
                                        return;
                                    }
                                    this.run_bg_action(
                                        format!("Migrating {rt} globals from {from_ver}..."),
                                        cx,
                                        move |_, _| backend::globals_migrate(&rt, &from_ver),
                                    );
                                }),
                            )),
                    ),
            )
            .into_any_element()
    }

    fn render_pins_tab(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
        palette: &ThemePalette,
    ) -> AnyElement {
        let busy = self.busy_label.is_some();

        let mut pin_rows = div().flex().flex_col().gap(px(8.0));
        if self.snapshot.pins.is_empty() {
            pin_rows = pin_rows.child(
                div()
                    .p(px(12.0))
                    .rounded(px(8.0))
                    .bg(palette.bg_surface_alt)
                    .text_xs()
                    .text_color(palette.text_muted)
                    .child("No version pin files (.synapse-version, .node-version, .nvmrc, .bun-version, .php-version) found from this directory upward."),
            );
        } else {
            for pin in &self.snapshot.pins {
                pin_rows = pin_rows.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(px(12.0))
                        .py(px(9.0))
                        .rounded(px(8.0))
                        .bg(palette.bg_surface_alt)
                        .border_1()
                        .border_color(palette.border_subtle)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(palette.text_primary)
                                        .child(pin.runtime.clone()),
                                )
                                .child(badge(pin.version.clone(), BadgeTone::Accent, palette)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(palette.text_muted)
                                .child(pin.source_file.clone()),
                        ),
                );
            }
        }

        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(
                section_card(palette)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(palette.text_primary)
                            .child("Project Directory"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(self.render_input_field(
                                "pin-dir-input",
                                TextFieldKind::PinDir,
                                "/path/to/project",
                                window,
                                cx,
                                palette,
                            ))
                            .child(pill_button(
                                "load-pin-dir-btn",
                                "Load Directory",
                                ButtonVariant::Secondary,
                                busy,
                                palette,
                                cx.listener(|this, _ev, _window, cx| this.reload_snapshot(cx)),
                            ))
                            .child(pill_button(
                                "apply-all-pins-btn",
                                "Apply All Pins (use)",
                                ButtonVariant::Primary,
                                busy || self.snapshot.pins.is_empty(),
                                palette,
                                cx.listener(|this, _ev, _window, cx| {
                                    this.run_bg_action("Applying project pins...", cx, |_, dir| {
                                        backend::use_all_pinned_in_dir(&dir)
                                    });
                                }),
                            )),
                    ),
            )
            .child(
                section_card(palette)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(palette.text_primary)
                            .child("Pin Runtime in .synapse-version"),
                    )
                    .child(self.render_runtime_chips(
                        "pin-rt-chip",
                        ALL_RUNTIMES,
                        &self.selected_pin_runtime,
                        cx,
                        palette,
                        |this, rt| {
                            this.selected_pin_runtime = rt.to_string();
                        },
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(self.render_input_field(
                                "pin-version-input",
                                TextFieldKind::PinVersion,
                                "Version or alias to pin (e.g. lts, 22, 8.3)",
                                window,
                                cx,
                                palette,
                            ))
                            .child(pill_button(
                                "pin-runtime-btn",
                                "Pin Version",
                                ButtonVariant::Primary,
                                busy,
                                palette,
                                cx.listener(|this, _ev, _window, cx| this.trigger_pin(cx)),
                            )),
                    ),
            )
            .child(
                section_card(palette)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(palette.text_primary)
                            .child("Detected Version Pins"),
                    )
                    .child(pin_rows),
            )
            .into_any_element()
    }
}

impl Render for SynapseApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = ThemePalette::for_mode(self.theme_mode);
        let banner = self.render_banner(cx, &palette);

        let body = match self.active_tab {
            NavTab::Runtimes => self.render_runtimes_tab(window, cx, &palette),
            NavTab::Services => self.render_services_tab(cx, &palette),
            NavTab::Globals => self.render_globals_tab(window, cx, &palette),
            NavTab::Pins => self.render_pins_tab(window, cx, &palette),
        };

        div()
            .flex()
            .size_full()
            .bg(palette.bg_app)
            .text_color(palette.text_primary)
            .child(self.render_sidebar(cx, &palette))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .h_full()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(10.0))
                            .px(px(24.0))
                            .py(px(18.0))
                            .border_b_1()
                            .border_color(palette.border)
                            .bg(palette.bg_surface)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(2.0))
                                            .child(
                                                div()
                                                    .text_xl()
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(palette.text_primary)
                                                    .child(self.active_tab.label()),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(palette.text_secondary)
                                                    .child(self.active_tab.subtitle()),
                                            ),
                                    )
                                    .child(badge(
                                        self.theme_mode.label(),
                                        BadgeTone::Neutral,
                                        &palette,
                                    )),
                            )
                            .when_some(banner, |el, b| el.child(b)),
                    )
                    .child(
                        div()
                            .id("main-scroll-body")
                            .flex_1()
                            .overflow_y_scroll()
                            .p(px(24.0))
                            .child(body),
                    ),
            )
    }
}
