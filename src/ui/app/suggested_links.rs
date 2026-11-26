//! Links an agent leaves with `suggest_navigation`: a card at the bottom right of its own
//! terminal. The view never moves; the user opens a link when they choose, or dismisses it.
use super::*;
use crate::model::{SuggestedLink, SuggestedLinkTarget};
use crate::services::navigation_links::{self, SuggestNavigationRequest};

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SuggestNavigationPayload {
    request: SuggestNavigationRequest,
    #[serde(default)]
    caller_terminal_id: Option<Uuid>,
    /// Set by `open_*` calls from an agent: their link joins the ones already shown.
    #[serde(default)]
    append: bool,
}

/// What the card shows for one link.
struct LinkView {
    icon: LinkIcon,
    title: String,
    detail: String,
}

#[derive(Clone, Copy)]
enum LinkIcon {
    Agent(AgentKind),
    Project,
    Group,
}

impl BlackholesApp {
    pub(super) fn suggest_navigation(
        &mut self,
        payload: SuggestNavigationPayload,
        cx: &mut Context<Self>,
    ) -> Result<serde_json::Value> {
        let request = payload.request;
        request.validate()?;
        let caller = payload.caller_terminal_id
            .filter(|id| self.session.terminals.iter().any(|terminal| terminal.id == *id))
            .context("suggest_navigation shows its links in the calling Blackholes terminal; call it from an agent running in one")?;
        if request.clear {
            let cleared = self.session.terminal_links.remove(&caller).map_or(0, |links| links.len());
            self.persist_session();
            cx.notify();
            return Ok(serde_json::json!({ "accepted": true, "terminalId": caller, "cleared": cleared }));
        }
        let resolve_all = |app: &Self| request.links.iter()
            .map(|link| navigation_links::resolve(link, &app.session, &app.workspaces, &app.tasks, caller))
            .collect::<Result<Vec<_>>>();
        // A project or group another process just created is not loaded yet.
        let targets = match resolve_all(self) {
            Ok(targets) => targets,
            Err(_) => {
                self.reload_external_data(cx);
                resolve_all(self)?
            }
        };
        let now = Utc::now();
        let mut links: Vec<SuggestedLink> = Vec::new();
        for (target, link) in targets.into_iter().zip(&request.links) {
            if !links.iter().any(|existing| existing.target == target) {
                links.push(SuggestedLink {
                    id: Uuid::new_v4(),
                    target,
                    reason: navigation_links::clean_reason(link.reason.as_deref()),
                    created_at: now,
                });
            }
        }
        Ok(self.store_links(caller, links, payload.append, cx))
    }

    /// Shows links in the caller's terminal, replacing its links or after them.
    pub(super) fn store_links(&mut self, caller: Uuid, links: Vec<SuggestedLink>, append: bool, cx: &mut Context<Self>) -> serde_json::Value {
        let links = if append {
            navigation_links::append(self.session.terminal_links.remove(&caller).unwrap_or_default(), links)
        } else {
            links
        };
        let shown: Vec<serde_json::Value> = links.iter().filter_map(|link| {
            let view = self.link_view(link.target)?;
            let (kind, id) = match link.target {
                SuggestedLinkTarget::Session { terminal_id } => ("session", terminal_id),
                SuggestedLinkTarget::Project { project_id } => ("project", project_id),
                SuggestedLinkTarget::WorktreeGroup { group_id } => ("worktreeGroup", group_id),
            };
            Some(serde_json::json!({ "kind": kind, "id": id, "title": view.title, "detail": view.detail, "reason": link.reason }))
        }).collect();
        self.session.terminal_links.insert(caller, links);
        self.persist_session();
        cx.notify();
        serde_json::json!({ "accepted": true, "terminalId": caller, "links": shown })
    }

    /// Name and place of a link's target, or `None` once it is gone.
    fn link_view(&self, target: SuggestedLinkTarget) -> Option<LinkView> {
        let project_name = |id: Uuid| self.workspaces.iter()
            .find(|workspace| workspace.id == id).map(|workspace| workspace.label().to_string());
        match target {
            SuggestedLinkTarget::Session { terminal_id } => {
                let terminal = self.session.terminals.iter().find(|terminal| terminal.id == terminal_id)?;
                let group = terminal.task_id.and_then(|id| self.tasks.iter().find(|task| task.id == id));
                let place = match (project_name(terminal.workspace_id), group) {
                    (Some(project), Some(group)) => Some(format!("{project} / {}", group.title)),
                    (project, group) => project.or_else(|| group.map(|group| group.title.clone())),
                };
                let running = self.terminals.contains_key(&terminal_id)
                    && !matches!(terminal.state, SessionState::Restored | SessionState::Exited);
                let mut detail = vec![terminal.agent.label().to_string()];
                detail.extend(place);
                if !running {
                    detail.push(self.tr("stopped", "detenida").to_string());
                }
                Some(LinkView {
                    icon: LinkIcon::Agent(terminal.agent),
                    title: agent_sections::shown_name(terminal),
                    detail: detail.join(" · "),
                })
            }
            SuggestedLinkTarget::Project { project_id } => Some(LinkView {
                icon: LinkIcon::Project,
                title: project_name(project_id)?,
                detail: self.tr("Project", "Proyecto").to_string(),
            }),
            SuggestedLinkTarget::WorktreeGroup { group_id } => {
                let group = self.tasks.iter().find(|task| task.id == group_id)?;
                let mut detail = vec![self.tr("Task", "Tarea").to_string()];
                detail.extend(project_name(group.workspace_id));
                Some(LinkView { icon: LinkIcon::Group, title: group.title.clone(), detail: detail.join(" · ") })
            }
        }
    }

    /// Opens one link and removes it; the terminal's other links stay.
    fn open_suggested_link(&mut self, caller: Uuid, link_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let Some(links) = self.session.terminal_links.get_mut(&caller) else { return };
        let Some(index) = links.iter().position(|link| link.id == link_id) else { return };
        let link = links.remove(index);
        if links.is_empty() {
            self.session.terminal_links.remove(&caller);
        }
        match link.target {
            SuggestedLinkTarget::Session { terminal_id } => self.focus_terminal(terminal_id, window, cx),
            SuggestedLinkTarget::Project { project_id } => self.open_project_from_navigation(project_id, cx),
            SuggestedLinkTarget::WorktreeGroup { group_id } => self.open_task_from_navigation(group_id, cx),
        }
        self.persist_session();
        self.hydrate_navigation(cx);
        cx.notify();
    }

    fn dismiss_suggested_links(&mut self, caller: Uuid, cx: &mut Context<Self>) {
        if self.session.terminal_links.remove(&caller).is_some() {
            self.persist_session();
            cx.notify();
        }
    }

    /// The card over the bottom right of the terminal that received the links, if any are still
    /// there to open. It never takes focus.
    pub(super) fn render_suggested_links(&self, terminal_id: Uuid, cx: &mut Context<Self>) -> Option<AnyElement> {
        let links: Vec<(Uuid, LinkView, Option<String>)> = self.session.terminal_links.get(&terminal_id)?
            .iter()
            .filter_map(|link| Some((link.id, self.link_view(link.target)?, link.reason.clone())))
            .collect();
        if links.is_empty() {
            return None;
        }
        let border = rgb(0x5b58a6);
        let background = rgb(0x24233d);
        let row_hover = rgb(0x302e55);
        let title_color = rgb(0xf4f3fd);
        let muted = rgb(0xbdbbdb);
        let close_hover = rgb(0x3e3c6a);
        let button = rgb(0x4d4aa6);
        let button_hover = rgb(0x605cc4);
        let heading = if links.len() == 1 {
            self.tr("Link from this agent", "Enlace de este agente")
        } else {
            self.tr("Links from this agent", "Enlaces de este agente")
        };
        let open_label = self.tr("Open", "Abrir");
        let weak = cx.weak_entity();
        let weak_dismiss = weak.clone();

        let mut card = v_flex()
            .id(SharedString::from(format!("suggested-links-{terminal_id}")))
            .absolute()
            .right(px(16.))
            .bottom(px(16.))
            .w(px(340.))
            .max_w(gpui::relative(0.92))
            .p_2()
            .gap_1()
            .rounded(px(10.))
            .border_1()
            .border_color(border)
            .bg(background)
            .shadow_lg()
            .occlude()
            // The terminal below must not take the press or the focus.
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                h_flex()
                    .pl_2()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(muted)
                            .child(heading.to_string()),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("dismiss-suggested-links-{terminal_id}")))
                            .size(px(24.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .text_color(muted)
                            .hover(move |style| style.bg(close_hover).text_color(rgb(0xffffff)))
                            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                let _ = weak_dismiss.update(cx, |app, cx| app.dismiss_suggested_links(terminal_id, cx));
                            })
                            .child(Icon::new(AppIcon::X).with_size(px(13.))),
                    ),
            );

        for (link_id, view, reason) in links {
            let weak_open = weak.clone();
            let icon = match view.icon {
                LinkIcon::Agent(agent) => agent_icon(agent),
                LinkIcon::Project => div().text_color(rgb(0x8bb9ff))
                    .child(Icon::new(AppIcon::Layers3).with_size(px(15.))).into_any_element(),
                LinkIcon::Group => div().text_color(rgb(0x8bb9ff))
                    .child(Icon::new(AppIcon::ListTodo).with_size(px(15.))).into_any_element(),
            };
            let icon_background = match view.icon {
                LinkIcon::Agent(AgentKind::Codex) => rgb(0x121916),
                LinkIcon::Agent(_) => rgb(0xf1f0fb),
                LinkIcon::Project | LinkIcon::Group => rgb(0x17253b),
            };
            card = card.child(
                h_flex()
                    .id(SharedString::from(format!("suggested-link-{link_id}")))
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap_3()
                    .p_2()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .hover(move |style| style.bg(row_hover))
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        let _ = weak_open.update(cx, |app, cx| app.open_suggested_link(terminal_id, link_id, window, cx));
                    })
                    .child(
                        div()
                            .size(px(32.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(icon_background)
                            .child(icon),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_size(px(13.))
                                    .text_color(title_color)
                                    .line_clamp(2)
                                    .child(view.title),
                            )
                            .child(div().text_size(px(11.)).text_color(muted).truncate().child(view.detail))
                            .when_some(reason, |this, reason| {
                                this.child(div().text_size(px(12.)).text_color(muted).line_clamp(2).child(reason))
                            }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .px_2()
                            .py_1()
                            .rounded(px(6.))
                            .bg(button)
                            .hover(move |style| style.bg(button_hover))
                            .text_size(px(12.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(0xffffff))
                            .child(open_label.to_string()),
                    ),
            );
        }
        Some(card.into_any_element())
    }
}
