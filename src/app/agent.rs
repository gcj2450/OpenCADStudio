//! Native Codex app-server integration for OpenCADStudio.
//!
//! The Codex runtime lives on a dedicated Tokio thread. The panel and CAD
//! drawing remain in the Iced process; dynamic tool calls are dispatched on
//! the UI thread through `control_request` and returned over in-memory
//! channels.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use codex_app_server_client::{
    start_embedded_surface_client, AppServerEvent, InProcessAppServerClient,
};
use codex_app_server_protocol::{
    Account, AskForApproval, CancelLoginAccountParams, CancelLoginAccountResponse, ClientRequest,
    CommandExecutionApprovalDecision, CommandExecutionRequestApprovalResponse,
    DynamicToolCallOutputContentItem, DynamicToolCallStatus, DynamicToolFunctionSpec,
    DynamicToolSpec, FileChangeApprovalDecision, FileChangeRequestApprovalResponse,
    GetAccountParams, GetAccountRateLimitsResponse, GetAccountResponse, LoginAccountParams,
    LoginAccountResponse, LogoutAccountResponse, ModelListParams, ModelListResponse,
    PermissionGrantScope, PermissionsRequestApprovalResponse, RequestId, SandboxMode,
    ServerNotification, ServerRequest, ThreadItem, ThreadListParams, ThreadListResponse,
    ThreadResumeParams, ThreadResumeResponse, ThreadStartParams, ThreadStartResponse,
    ToolRequestUserInputAnswer, ToolRequestUserInputParams, ToolRequestUserInputResponse,
    TurnInterruptParams, TurnPlanUpdatedNotification, TurnStartParams, UserInput,
};
use iced::futures::channel::mpsc;
use iced::futures::lock::Mutex;
use iced::futures::StreamExt;
use iced::widget::{button, column, container, row, scrollable, text, text_editor, text_input};
use iced::{Background, Border, Element, Fill, Task, Theme};
use serde_json::{json, Value};

use super::{Message, OpenCADStudio};

#[derive(Clone, Debug)]
pub(super) enum AgentEvent {
    Questions {
        request_id: Value,
        params: ToolRequestUserInputParams,
    },
    Plan(TurnPlanUpdatedNotification),
    AccountLoaded(GetAccountResponse),
    AccountStatus(String),
    LoginStarted {
        id: String,
        url: String,
        code: Option<String>,
    },
    LoginCanceled(String),
    LoginCompleted {
        id: Option<String>,
        success: bool,
        error: Option<String>,
    },
    UsageLoaded(GetAccountRateLimitsResponse),
    AccountCleared,
    Status(String),
    AssistantDelta(String),
    TurnRetrying(String),
    TurnComplete,
    ConversationList(Vec<AgentConversation>),
    ConversationLoaded {
        id: String,
        title: String,
        messages: Vec<ChatMessage>,
    },
    ConversationStarted {
        id: String,
    },
    ModelCatalog {
        models: Vec<(String, String)>,
    },
    ApprovalRequest(AgentApproval),
    ToolCall {
        server_request_id: Value,
        call_id: String,
        tool: String,
        arguments: Value,
    },
    Error(String),
}

#[derive(Clone, Default)]
pub(super) struct AgentSecret(pub String);

impl std::fmt::Debug for AgentSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum AgentAccountAction {
    Refresh,
    LoginDevice,
    LoginBrowser,
    LoginApiKey,
    CancelLogin,
    Logout,
    RefreshUsage,
}

struct AgentLogin {
    id: String,
    url: String,
    code: Option<String>,
}

struct AgentQuestions {
    request_id: Value,
    params: ToolRequestUserInputParams,
    answers: Vec<AgentSecret>,
}

impl AgentQuestions {
    fn response(&self, skip: bool) -> Option<ToolRequestUserInputResponse> {
        if !skip && self.answers.iter().any(|answer| answer.0.trim().is_empty()) {
            return None;
        }
        Some(ToolRequestUserInputResponse {
            answers: self
                .params
                .questions
                .iter()
                .zip(&self.answers)
                .map(|(question, answer)| {
                    (
                        question.id.clone(),
                        ToolRequestUserInputAnswer {
                            answers: if skip {
                                Vec::new()
                            } else {
                                vec![answer.0.clone()]
                            },
                        },
                    )
                })
                .collect(),
        })
    }
}

#[derive(Clone, Debug)]
pub(super) struct AgentConversation {
    id: String,
    title: String,
    preview: String,
}

#[derive(Clone, Debug)]
pub(super) struct AgentApproval {
    request_id: Value,
    title: String,
    details: String,
    approve_response: Value,
    reject_response: Value,
}

#[derive(Clone, Debug)]
pub(super) struct ChatMessage {
    role: &'static str,
    text: String,
    content: text_editor::Content,
}

impl ChatMessage {
    fn new(role: &'static str, text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            role,
            content: text_editor::Content::with_text(&text),
            text,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct PendingToolCall {
    server_request_id: Value,
    operation_id: String,
    capture_path: Option<PathBuf>,
    capture_max_dimension: u32,
    batch_result_id: Option<String>,
}

enum AgentCommand {
    Account(AgentAccountAction),
    LoginApiKey(AgentSecret),
    CancelLogin(String),
    Prompt {
        text: String,
        cwd: PathBuf,
        model: Option<String>,
        effort: Option<String>,
    },
    ListConversations,
    LoadModels,
    ResumeConversation {
        id: String,
        cwd: PathBuf,
    },
    NewConversation,
    ResolveRequest {
        request_id: Value,
        result: Value,
    },
    Stop,
}

pub(super) struct AgentPanelState {
    questions: Vec<AgentQuestions>,
    plan: Option<TurnPlanUpdatedNotification>,
    pub(super) api_key_input: AgentSecret,
    account: Option<Account>,
    account_status: String,
    account_pending: bool,
    login: Option<AgentLogin>,
    usage: Option<GetAccountRateLimitsResponse>,
    polling: bool,
    pub input: String,
    pub(super) model_input: String,
    pub(super) selected_effort: String,
    messages: Vec<ChatMessage>,
    status: String,
    busy: bool,
    last_turn_error: Option<String>,
    streaming_message: bool,
    history_open: bool,
    settings_open: bool,
    conversations: Vec<AgentConversation>,
    model_catalog: Vec<(String, String)>,
    active_thread_id: Option<String>,
    commands: Option<mpsc::UnboundedSender<AgentCommand>>,
    events: Option<Arc<Mutex<mpsc::UnboundedReceiver<AgentEvent>>>>,
    pub(super) pending_tools: Vec<PendingToolCall>,
    pending_approvals: Vec<AgentApproval>,
    batch_results: HashMap<String, Value>,
    document_context: Option<Value>,
    next_operation: u64,
}

impl Default for AgentPanelState {
    fn default() -> Self {
        Self {
            questions: Vec::new(),
            plan: None,
            api_key_input: AgentSecret::default(),
            account: None,
            account_status: "Account not loaded".into(),
            account_pending: false,
            login: None,
            usage: None,
            polling: false,
            input: String::new(),
            model_input: String::new(),
            selected_effort: "medium".to_string(),
            messages: Vec::new(),
            status: "Codex is ready when you are".to_string(),
            busy: false,
            last_turn_error: None,
            streaming_message: false,
            history_open: false,
            settings_open: false,
            conversations: Vec::new(),
            model_catalog: Vec::new(),
            active_thread_id: None,
            commands: None,
            events: None,
            pending_tools: Vec::new(),
            pending_approvals: Vec::new(),
            batch_results: HashMap::new(),
            document_context: None,
            next_operation: 1,
        }
    }
}

impl AgentPanelState {
    pub(super) fn apply_transcript_action(
        &mut self,
        index: usize,
        action: text_editor::Action,
    ) {
        if let Some(message) = self.messages.get_mut(index) {
            // The transcript is read-only. Navigation and selection actions
            // are still applied so mouse selection and Ctrl+A/Ctrl+C work,
            // while insert/delete/paste actions are ignored.
            if !action.is_edit() {
                message.content.perform(action);
            }
        }
    }

    fn append_assistant_delta(&mut self, delta: &str) {
        if !self.streaming_message
            || self
                .messages
                .last()
                .is_none_or(|message| message.role != "assistant")
        {
            self.messages.push(ChatMessage::new("assistant", String::new()));
            self.streaming_message = true;
        }
        if let Some(message) = self.messages.last_mut() {
            message.text.push_str(delta);
            // Rebuild the editor content while streaming so the visible text
            // and the selectable buffer stay in sync. Selection is only
            // meaningful after a turn has finished and is therefore safe to
            // reset while new deltas arrive.
            message.content = text_editor::Content::with_text(&message.text);
        }
    }

    fn start_service(&mut self) {
        if self.commands.is_some() {
            return;
        }

        let (command_tx, command_rx) = mpsc::unbounded();
        let (event_tx, event_rx) = mpsc::unbounded();
        let event_tx = event_tx.clone();
        // Embedded Codex request futures need more than the Windows default
        // stack even when the app is launched without Cargo's RUST_MIN_STACK.
        let agent_stack_size = 64 * 1024 * 1024;
        let thread = std::thread::Builder::new()
            .name("ocs-codex-agent".to_string())
            .stack_size(agent_stack_size)
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .thread_stack_size(agent_stack_size)
                    .enable_all()
                    .build();
                match runtime {
                    Ok(runtime) => runtime.block_on(run_agent(command_rx, event_tx)),
                    Err(error) => {
                        let _ = event_tx.unbounded_send(AgentEvent::Error(format!(
                            "Could not start the embedded Codex runtime: {error}"
                        )));
                    }
                }
            });

        match thread {
            Ok(_) => {
                self.commands = Some(command_tx);
                self.events = Some(Arc::new(Mutex::new(event_rx)));
                self.status = "Starting the embedded Codex runtime…".to_string();
            }
            Err(error) => {
                self.status = format!("Could not start the Codex thread: {error}");
            }
        }
    }

    fn poll_task(&mut self) -> Task<Message> {
        if self.polling {
            return Task::none();
        }
        let Some(events) = self.events.clone() else {
            return Task::none();
        };
        self.polling = true;
        Task::perform(
            async move {
                let mut events = events.lock().await;
                events.next().await
            },
            Message::AgentEvent,
        )
    }

    fn send_command(&mut self, command: AgentCommand) -> bool {
        self.commands
            .as_ref()
            .is_some_and(|sender| sender.unbounded_send(command).is_ok())
    }

    fn retain_batch_result(&mut self, request_id: String, result: Value) {
        if self.batch_results.len() >= 64 && !self.batch_results.contains_key(&request_id) {
            if let Some(oldest_key) = self.batch_results.keys().next().cloned() {
                self.batch_results.remove(&oldest_key);
            }
        }
        self.batch_results.insert(request_id, result);
    }

    fn reply_to_tool(&mut self, request_id: Value, result: Value, image: Option<String>) {
        self.messages.push(ChatMessage::new(
            "tool-result",
            bounded_transcript_value(&result, 2_000),
        ));
        let mut content_items = vec![json!({
            "type": "inputText",
            "text": result.to_string(),
        })];
        if let Some(image_url) = image {
            content_items.push(json!({
                "type": "inputImage",
                "imageUrl": image_url,
            }));
        }
        let response = json!({
            "contentItems": content_items,
            "success": result["ok"].as_bool() != Some(false)
                && result["valid"].as_bool() != Some(false),
        });
        let _ = self.send_command(AgentCommand::ResolveRequest {
            request_id,
            result: response,
        });
    }

    fn active_cwd(&self, app: &OpenCADStudio) -> PathBuf {
        app.tabs[app.active_tab]
            .current_path
            .as_ref()
            .and_then(|path| path.parent().map(PathBuf::from))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    fn selected_model(&self) -> Option<String> {
        let model = self.model_input.trim();
        (!model.is_empty()).then(|| model.to_string())
    }

    fn selected_effort(&self) -> Option<String> {
        (!self.selected_effort.is_empty()).then(|| self.selected_effort.clone())
    }

    fn poll_for_command(&mut self, command: AgentCommand) -> Task<Message> {
        self.start_service();
        if self.send_command(command) {
            self.poll_task()
        } else {
            self.status = "The embedded Codex runtime is not available".to_string();
            self.account_pending = false;
            Task::none()
        }
    }

    pub fn view<'a>(&'a self, width: f32, chrome: crate::ui::dock::Chrome) -> Element<'a, Message> {
        let title = crate::ui::dock::title_bar(
            crate::ui::dock::PanelId::Agent,
            crate::t!("CAD Agent").into_owned(),
            chrome,
        );

        let mut transcript = column![].spacing(8).width(Fill);
        for (index, message) in self.messages.iter().enumerate() {
            let label = match message.role {
                "user" => "You",
                "tool" => "CAD tool",
                "tool-result" => "Tool result",
                _ => "Codex",
            };
            transcript = transcript.push(
                container(column![
                    text(label).size(10),
                    text_editor::TextEditor::new(&message.content)
                        .on_action(move |action| Message::AgentTranscriptAction(
                            index,
                            action,
                        ))
                        .size(12)
                        .padding(0)
                        .height(iced::Length::Shrink)
                ])
                .width(Fill)
                .padding(8)
                .style(|theme: &Theme| container::Style {
                    background: Some(Background::Color(theme.palette().background.weak.color)),
                    border: Border {
                        radius: 4.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            );
        }
        if self.messages.is_empty() {
            transcript = transcript.push(
                container(
                    text("Ask about the active drawing, inspect entities, or describe a CAD edit.")
                        .size(12)
                        .style(crate::ui::style::common::muted_style),
                )
                .width(Fill)
                .padding(8),
            );
        }

        let conversation_controls = row![
            button(text("New chat").size(11))
                .on_press_maybe((!self.busy).then_some(Message::AgentNewConversation))
                .padding([6, 8]),
            button(text("History").size(11))
                .on_press_maybe((!self.busy).then_some(Message::AgentToggleHistory))
                .padding([6, 8]),
            button(text("Settings").size(11))
                .on_press_maybe((!self.busy).then_some(Message::AgentToggleSettings))
                .padding([6, 8]),
            text(
                self.active_thread_id
                    .as_ref()
                    .map(|id| format!("Conversation {}", id.chars().take(8).collect::<String>()))
                    .unwrap_or_else(|| "New conversation".to_string()),
            )
            .size(9)
            .style(crate::ui::style::common::muted_style)
        ]
        .spacing(6)
        .width(Fill);

        let quick_actions = row![
            button(text("Inspect drawing").size(10))
                .on_press_maybe((!self.busy).then_some(Message::AgentQuickPrompt(
                    "Read the active drawing with ocs_read and summarize its document identity, layers, entity counts, and current selection. Do not modify anything.".into(),
                )))
                .padding([5, 7]),
            button(text("Recognize structure").size(10))
                .on_press_maybe((!self.busy).then_some(Message::AgentQuickPrompt(
                    "Recognize the active drawing structure with native ocs_read op recognize. Use model-space evidence, report complete, page cursors, and reviewItems, and do not modify anything.".into(),
                )))
                .padding([5, 7]),
            button(text("Analyze selection").size(10))
                .on_press_maybe((!self.busy).then_some(Message::AgentQuickPrompt(
                    "Inspect the current CAD selection and explain each selected entity's type, layer, geometry, and likely role. If there is no selection, say so. Do not modify anything.".into(),
                )))
                .padding([5, 7]),
        ]
        .spacing(4)
        .width(Fill);

        let history: Element<'_, Message> = if self.history_open {
            let mut entries = column![text("Recent conversations").size(10)].spacing(4);
            if self.conversations.is_empty() {
                entries = entries.push(text("No saved conversations yet").size(10));
            }
            for conversation in self.conversations.iter().take(16) {
                let title = if conversation.title.trim().is_empty() {
                    conversation.preview.as_str()
                } else {
                    conversation.title.as_str()
                };
                entries = entries.push(
                    button(
                        column![
                            text(title).size(10),
                            text(conversation.preview.as_str())
                                .size(9)
                                .style(crate::ui::style::common::muted_style),
                        ]
                        .spacing(2),
                    )
                    .on_press_maybe(
                        (!self.busy)
                            .then_some(Message::AgentResumeConversation(conversation.id.clone())),
                    )
                    .padding([5, 7])
                    .width(Fill),
                );
            }
            container(scrollable(entries).height(140))
                .width(Fill)
                .padding([4, 8])
                .into()
        } else {
            container(text("")).height(0).into()
        };

        let settings: Element<'_, Message> = if self.settings_open {
            let mut models = row![].spacing(4);
            for (id, name) in self.model_catalog.iter().take(6) {
                models = models.push(
                    button(text(name.as_str()).size(9))
                        .on_press(Message::AgentChooseModel(id.clone()))
                        .padding([4, 6]),
                );
            }
            container(
                column![
                    self.account_view(),
                    text("Model used for new and resumed turns").size(10),
                    text_input("Default model", &self.model_input)
                        .on_input(Message::AgentModelInput)
                        .size(10)
                        .padding(6),
                    models,
                    text("Reasoning effort").size(10),
                    row![
                        button(text("Low").size(9))
                            .on_press(Message::AgentSetEffort("low".into()))
                            .padding([4, 6]),
                        button(text("Medium").size(9))
                            .on_press(Message::AgentSetEffort("medium".into()))
                            .padding([4, 6]),
                        button(text("High").size(9))
                            .on_press(Message::AgentSetEffort("high".into()))
                            .padding([4, 6]),
                        text(self.selected_effort.as_str()).size(9),
                    ]
                    .spacing(4),
                ]
                .spacing(5),
            )
            .width(Fill)
            .padding([5, 8])
            .into()
        } else {
            container(text("")).height(0).into()
        };

        let mut approvals = column![].spacing(6).width(Fill);
        for (index, approval) in self.pending_approvals.iter().enumerate() {
            approvals = approvals.push(
                container(
                    column![
                        text(approval.title.as_str()).size(11),
                        text(approval.details.as_str()).size(10),
                        row![
                            button(text("Allow once").size(10))
                                .on_press(Message::AgentResolveApproval(index, true))
                                .padding([5, 8]),
                            button(text("Reject").size(10))
                                .on_press(Message::AgentResolveApproval(index, false))
                                .padding([5, 8]),
                        ]
                        .spacing(6),
                    ]
                    .spacing(5),
                )
                .width(Fill)
                .padding(7)
                .style(|theme: &Theme| container::Style {
                    background: Some(Background::Color(theme.palette().background.weak.color)),
                    border: Border {
                        radius: 4.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            );
        }

        let composer = row![
            text_input("Ask the CAD agent…", &self.input)
                .on_input(Message::AgentInput)
                .on_submit(Message::AgentSend)
                .size(12)
                .padding(8)
                .width(Fill),
            if self.busy {
                button(text("Stop").size(11))
                    .on_press(Message::AgentStop)
                    .padding([7, 10])
            } else {
                button(text("Send").size(11))
                    .on_press(Message::AgentSend)
                    .padding([7, 10])
            }
        ]
        .spacing(6)
        .width(Fill);

        let body = column![
            container(conversation_controls).width(Fill).padding([4, 8]),
            container(quick_actions).width(Fill).padding([0, 8]),
            history,
            settings,
            container(approvals).width(Fill).padding([2, 8]),
            scrollable(self.question_view()).height(iced::Length::Shrink),
            self.plan_view(),
            container(
                text(self.status.as_str())
                    .size(10)
                    .style(crate::ui::style::common::muted_style)
            )
            .width(Fill)
            .padding([2, 8]),
            scrollable(container(transcript).width(Fill).padding([4, 8]))
                .height(Fill)
                .width(Fill),
            container(composer).width(Fill).padding([5, 8]),
        ]
        .spacing(4)
        .height(Fill);

        crate::ui::dock::frame(title, body, width)
    }

    fn account_view(&self) -> Element<'_, Message> {
        let enabled = !self.busy && !self.account_pending;
        let action = |label: &'static str, action| {
            button(text(label).size(10))
                .on_press_maybe(enabled.then_some(Message::AgentAccount(action)))
                .padding([4, 6])
        };
        let mut content = column![
            text("Account").size(11),
            text(self.account_status.as_str()).size(10),
            row![
                action("Refresh", AgentAccountAction::Refresh),
                action("Usage", AgentAccountAction::RefreshUsage),
                action("Sign out", AgentAccountAction::Logout),
            ]
            .spacing(4),
        ]
        .spacing(5);
        if let Some(login) = &self.login {
            content = content.push(text("Complete sign-in in your browser").size(10));
            if let Some(code) = &login.code {
                content = content.push(text(format!("Device code: {code}")).size(12));
            }
            content = content.push(
                row![
                    button(text("Open sign-in page").size(10))
                        .on_press(Message::OpenUrl(login.url.clone()))
                        .padding([4, 6]),
                    action("Cancel", AgentAccountAction::CancelLogin),
                ]
                .spacing(4),
            );
        } else {
            content = content.push(
                row![
                    action("Device sign-in", AgentAccountAction::LoginDevice),
                    action("Browser sign-in", AgentAccountAction::LoginBrowser),
                ]
                .spacing(4),
            );
            content = content
                .push(
                    text_input("API key", &self.api_key_input.0)
                        .secure(true)
                        .on_input(|value| Message::AgentApiKeyInput(AgentSecret(value)))
                        .size(10)
                        .padding(6),
                )
                .push(action("Use API key", AgentAccountAction::LoginApiKey));
        }
        if let Some(usage) = &self.usage {
            for (name, window) in [
                ("Primary", &usage.rate_limits.primary),
                ("Secondary", &usage.rate_limits.secondary),
            ] {
                if let Some(window) = window {
                    content = content
                        .push(text(format!("{name}: {}% used", window.used_percent)).size(10));
                }
            }
            if usage.ordinary_usage_allowed == Some(false) {
                content = content.push(text("Included usage is currently unavailable").size(10));
            }
        }
        content.into()
    }
}

impl AgentPanelState {
    fn question_view(&self) -> Element<'_, Message> {
        let mut cards = column![].spacing(8).padding([0, 8]);
        for request in &self.questions {
            let mut card = column![].spacing(5);
            for (index, question) in request.params.questions.iter().enumerate() {
                card = card
                    .push(text(question.header.as_str()).size(11))
                    .push(text(question.question.as_str()).size(11));
                if let Some(options) = &question.options {
                    for option in options {
                        card = card.push(
                            button(column![
                                text(option.label.as_str()).size(10),
                                text(option.description.as_str()).size(9)
                            ])
                            .on_press(Message::AgentQuestionInput(
                                request.request_id.clone(),
                                index,
                                AgentSecret(option.label.clone()),
                            ))
                            .padding([4, 6]),
                        );
                    }
                }
                let id = request.request_id.clone();
                card = card.push(
                    text_input("Your answer", &request.answers[index].0)
                        .secure(question.is_secret)
                        .on_input(move |value| {
                            Message::AgentQuestionInput(id.clone(), index, AgentSecret(value))
                        })
                        .size(11)
                        .padding(6),
                );
            }
            card = card.push(
                row![
                    button(text("Submit answers").size(10))
                        .on_press_maybe(request.response(false).is_some().then(|| {
                            Message::AgentSubmitAnswers(request.request_id.clone(), false)
                        }))
                        .padding([4, 6]),
                    button(text("Skip").size(10))
                        .on_press(Message::AgentSubmitAnswers(
                            request.request_id.clone(),
                            true
                        ))
                        .padding([4, 6]),
                ]
                .spacing(5),
            );
            cards = cards.push(card);
        }
        cards.into()
    }

    fn plan_view(&self) -> Element<'_, Message> {
        let mut content = column![].spacing(3).padding([0, 8]);
        if let Some(plan) = &self.plan {
            if let Some(explanation) = &plan.explanation {
                content = content.push(text(explanation.as_str()).size(10));
            }
            for step in &plan.plan {
                content = content.push(text(format!("{:?}: {}", step.status, step.step)).size(10));
            }
        }
        content.into()
    }
}

impl OpenCADStudio {
    pub(super) fn toggle_agent_history(&mut self) -> Task<Message> {
        self.agent.history_open = !self.agent.history_open;
        if self.agent.history_open {
            self.agent.poll_for_command(AgentCommand::ListConversations)
        } else {
            Task::none()
        }
    }

    pub(super) fn toggle_agent_settings(&mut self) -> Task<Message> {
        self.agent.settings_open = !self.agent.settings_open;
        if self.agent.settings_open {
            self.agent.start_service();
            self.agent.account_pending = true;
            self.agent
                .send_command(AgentCommand::Account(AgentAccountAction::Refresh));
            self.agent.poll_for_command(AgentCommand::LoadModels)
        } else {
            Task::none()
        }
    }

    pub(super) fn agent_account_action(&mut self, action: AgentAccountAction) -> Task<Message> {
        if self.agent.busy || self.agent.account_pending {
            return Task::none();
        }
        let command = match action {
            AgentAccountAction::LoginApiKey => {
                let key = std::mem::take(&mut self.agent.api_key_input);
                if key.0.trim().is_empty() {
                    self.agent.account_status = "Enter an API key".into();
                    return Task::none();
                }
                AgentCommand::LoginApiKey(key)
            }
            AgentAccountAction::CancelLogin => {
                let Some(login) = &self.agent.login else {
                    return Task::none();
                };
                AgentCommand::CancelLogin(login.id.clone())
            }
            _ => AgentCommand::Account(action),
        };
        self.agent.account_pending = true;
        self.agent.account_status = "Updating account…".into();
        self.agent.poll_for_command(command)
    }

    pub(super) fn new_agent_conversation(&mut self) -> Task<Message> {
        if self.agent.busy {
            return Task::none();
        }
        self.agent.active_thread_id = None;
        self.agent.messages.clear();
        self.agent.pending_approvals.clear();
        self.agent.questions.clear();
        self.agent.plan = None;
        self.agent.document_context = None;
        self.agent.history_open = false;
        self.agent.poll_for_command(AgentCommand::NewConversation)
    }

    pub(super) fn resume_agent_conversation(&mut self, id: String) -> Task<Message> {
        if self.agent.busy {
            return Task::none();
        }
        let cwd = self.agent.active_cwd(self);
        self.agent.busy = true;
        self.agent.status = "Loading conversation history…".to_string();
        self.agent
            .poll_for_command(AgentCommand::ResumeConversation { id, cwd })
    }

    pub(super) fn resolve_agent_approval(&mut self, index: usize, approve: bool) -> Task<Message> {
        if index >= self.agent.pending_approvals.len() {
            return Task::none();
        }
        let approval = self.agent.pending_approvals.remove(index);
        let result = if approve {
            approval.approve_response
        } else {
            approval.reject_response
        };
        self.agent.poll_for_command(AgentCommand::ResolveRequest {
            request_id: approval.request_id,
            result,
        })
    }

    pub(super) fn send_agent_prompt(&mut self) -> Task<Message> {
        let prompt = self.agent.input.trim().to_string();
        if prompt.is_empty() || self.agent.busy {
            return Task::none();
        }
        self.agent.start_service();
        // Empty model means the runtime config, including custom providers.
        self.agent
            .send_command(AgentCommand::Account(AgentAccountAction::Refresh));
        self.agent.last_turn_error = None;
        self.agent.input.clear();
        self.agent.plan = None;
        self.agent
            .messages
            .push(ChatMessage::new("user", prompt.clone()));
        self.agent
            .messages
            .push(ChatMessage::new("assistant", String::new()));
        self.agent.streaming_message = true;
        self.agent.busy = true;
        self.agent.status = "Thinking…".to_string();
        let cwd = self.agent.active_cwd(self);
        if !self.agent.send_command(AgentCommand::Prompt {
            text: prompt,
            cwd,
            model: self.agent.selected_model(),
            effort: self.agent.selected_effort(),
        }) {
            self.agent.busy = false;
            self.agent.status = "The embedded Codex runtime is not available".to_string();
            return Task::none();
        }
        self.agent.poll_task()
    }

    pub(super) fn agent_quick_prompt(&mut self, prompt: String) -> Task<Message> {
        if self.agent.busy {
            return Task::none();
        }
        self.agent.input = prompt;
        self.send_agent_prompt()
    }

    pub(super) fn stop_agent_turn(&mut self) -> Task<Message> {
        let _ = self.agent.send_command(AgentCommand::Stop);
        self.agent.status = "Stopping…".to_string();
        Task::none()
    }

    pub(super) fn on_agent_event(&mut self, event: Option<AgentEvent>) -> Task<Message> {
        self.agent.polling = false;
        match event {
            Some(AgentEvent::Questions { request_id, params }) => {
                if !self
                    .agent
                    .questions
                    .iter()
                    .any(|request| request.request_id == request_id)
                {
                    let answers = vec![AgentSecret::default(); params.questions.len()];
                    self.agent.questions.push(AgentQuestions {
                        request_id,
                        params,
                        answers,
                    });
                }
            }
            Some(AgentEvent::Plan(plan)) => self.agent.plan = Some(plan),
            Some(AgentEvent::AccountLoaded(response)) => {
                self.agent.account_status = match &response.account {
                    Some(Account::Chatgpt { email, plan_type }) => {
                        format!("{} ({plan_type:?})", email.as_deref().unwrap_or("ChatGPT"))
                    }
                    Some(Account::ApiKey {}) => "API key configured".into(),
                    Some(Account::AmazonBedrock { .. }) => "Amazon Bedrock configured".into(),
                    None if response.requires_openai_auth => "Sign in to use the CAD agent".into(),
                    None => "Using the configured model provider".into(),
                };
                self.agent.account = response.account;
                self.agent.usage = None;
                self.agent.account_pending = false;
            }
            Some(AgentEvent::AccountStatus(status)) => {
                self.agent.account_status = status;
                self.agent.account_pending = false;
            }
            Some(AgentEvent::LoginStarted { id, url, code }) => {
                self.agent.login = Some(AgentLogin { id, url, code });
                self.agent.account_pending = false;
                self.agent.account_status = "Waiting for sign-in…".into();
            }
            Some(AgentEvent::LoginCanceled(id)) => {
                if self
                    .agent
                    .login
                    .as_ref()
                    .is_some_and(|login| login.id == id)
                {
                    self.agent.login = None;
                }
                self.agent.account_pending = false;
            }
            Some(AgentEvent::LoginCompleted { id, success, error }) => {
                if self
                    .agent
                    .login
                    .as_ref()
                    .is_some_and(|login| id.as_deref() == Some(login.id.as_str()))
                {
                    self.agent.login = None;
                    self.agent.account_pending = false;
                    self.agent.account_status = if success {
                        "Signed in".into()
                    } else {
                        error.unwrap_or_else(|| "Sign-in did not complete".into())
                    };
                    if success {
                        self.agent
                            .send_command(AgentCommand::Account(AgentAccountAction::Refresh));
                        self.agent.send_command(AgentCommand::LoadModels);
                    }
                }
            }
            Some(AgentEvent::UsageLoaded(usage)) => {
                self.agent.usage = Some(usage);
                self.agent.account_pending = false;
            }
            Some(AgentEvent::AccountCleared) => {
                self.agent.account = None;
                self.agent.login = None;
                self.agent.usage = None;
                self.agent.model_catalog.clear();
            }
            Some(AgentEvent::Status(status)) => self.agent.status = status,
            Some(AgentEvent::AssistantDelta(delta)) => {
                self.agent.append_assistant_delta(&delta);
            }
            Some(AgentEvent::TurnRetrying(detail)) => {
                self.agent.status = format!("连接中断，正在重试：{detail}");
            }
            Some(AgentEvent::TurnComplete) => {
                self.agent.busy = false;
                self.agent.questions.clear();
                self.agent.pending_approvals.clear();
                self.agent.streaming_message = false;
                self.agent.status = self.agent.last_turn_error.clone()
                    .unwrap_or_else(|| "Ready".to_string());
            }
            Some(AgentEvent::ConversationList(conversations)) => {
                self.agent.conversations = conversations;
            }
            Some(AgentEvent::ConversationLoaded {
                id,
                title,
                messages,
            }) => {
                self.agent.active_thread_id = Some(id);
                self.agent.messages = messages;
                self.agent.questions.clear();
                self.agent.pending_approvals.clear();
                self.agent.plan = None;
                self.agent.streaming_message = false;
                self.agent.busy = false;
                self.agent.history_open = false;
                self.agent.status = if title.is_empty() {
                    "Conversation history loaded".to_string()
                } else {
                    format!("Loaded: {title}")
                };
            }
            Some(AgentEvent::ConversationStarted { id }) => {
                self.agent.active_thread_id = (!id.is_empty()).then_some(id);
            }
            Some(AgentEvent::ModelCatalog {
                models,
            }) => {
                self.agent.model_catalog = models;
            }
            Some(AgentEvent::ApprovalRequest(approval)) => {
                self.agent.pending_approvals.push(approval);
            }
            Some(AgentEvent::Error(error)) => {
                let error = agent_error_message(&error);
                self.agent.busy = false;
                self.agent.streaming_message = false;
                self.agent.status = error.clone();
                // Turn errors are otherwise easy to miss in the status line;
                // keep the full server detail in the transcript and give the
                // common unauthenticated case a direct next step.
                if self.agent.last_turn_error.as_ref() != Some(&error) {
                    self.agent.append_assistant_delta(&format!("\n\n{error}"));
                    self.agent.streaming_message = false;
                }
                self.agent.last_turn_error = Some(error);
                // Keep draining the stream after an error. Fatal startup or
                // transport failures close it with `None`, which clears the
                // stale command sender so the next prompt can retry startup.
                return self.agent.poll_task();
            }
            Some(event @ AgentEvent::ToolCall { .. }) => {
                let task = self.execute_agent_tool(event);
                return Task::batch([task, self.agent.poll_task()]);
            }
            None => {
                self.agent.commands = None;
                self.agent.events = None;
                self.agent.busy = false;
                self.agent.account_pending = false;
                self.agent.login = None;
                self.agent.questions.clear();
                self.agent.status = "The embedded Codex runtime stopped".to_string();
                return Task::none();
            }
        }
        self.agent.poll_task()
    }

    pub(super) fn agent_question_input(
        &mut self,
        id: Value,
        index: usize,
        answer: AgentSecret,
    ) -> Task<Message> {
        if let Some(request) = self
            .agent
            .questions
            .iter_mut()
            .find(|request| request.request_id == id)
        {
            if let Some(value) = request.answers.get_mut(index) {
                *value = answer;
            }
        }
        Task::none()
    }

    pub(super) fn submit_agent_answers(&mut self, id: Value, skip: bool) -> Task<Message> {
        let Some(index) = self
            .agent
            .questions
            .iter()
            .position(|request| request.request_id == id)
        else {
            return Task::none();
        };
        let Some(response) = self.agent.questions[index].response(skip) else {
            return Task::none();
        };
        self.agent.questions.remove(index);
        let result = serde_json::to_value(response).unwrap_or_else(|_| json!({"answers":{}}));
        self.agent.poll_for_command(AgentCommand::ResolveRequest {
            request_id: id,
            result,
        })
    }

    fn execute_agent_tool(&mut self, event: AgentEvent) -> Task<Message> {
        let AgentEvent::ToolCall {
            server_request_id,
            call_id,
            tool,
            arguments,
        } = event
        else {
            return Task::none();
        };

        self.agent.messages.push(ChatMessage::new(
            "tool",
            format!("{tool}\n{}", bounded_transcript_value(&arguments, 1_200)),
        ));

        let mut capture_path = None;
        let mut capture_max_dimension = 1600;
        let request = match tool.as_str() {
            "ocs_sessions" => {
                let state = self.control_request(json!({ "op": "state" })).0;
                if state["ok"].as_bool() == Some(true) {
                    self.agent.document_context = Some(state.clone());
                }
                let session = json!({
                    "ok": true,
                    "session_id": super::control::session_id(),
                    "document_id": state["document_id"],
                    "revision": state["revision"],
                    "geometry_revision": state["geometry_revision"],
                    "camera_revision": state["camera_revision"],
                    "selection": state["selection"],
                    "documents": state["documents"],
                });
                self.agent
                    .reply_to_tool(server_request_id, json!({ "result": [session] }), None);
                return Task::none();
            }
            "ocs_read" => {
                let op = arguments["op"].as_str().unwrap_or("state");
                let parameters = arguments["parameters"]
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
                match op {
                    "guide" => {
                        let result = match parameters.get("name").and_then(Value::as_str) {
                            Some(name) => crate::mcp::guide_resource(name)
                                .map_err(|error| json!({ "ok": false, "error": error })),
                            None => Ok(json!({ "guides": crate::mcp::guide_resources() })),
                        };
                        self.agent.reply_to_tool(
                            server_request_id,
                            result.unwrap_or_else(|error| error),
                            None,
                        );
                        return Task::none();
                    }
                    "entity_schema" => {
                        let schema = parameters
                            .get("type")
                            .and_then(Value::as_str)
                            .map(crate::entity_creation::schema);
                        let result = match schema {
                            Some(Ok(schema)) => json!({ "ok": true, "schema": schema }),
                            Some(Err(error)) => json!({ "ok": false, "error": error }),
                            None => json!({
                                "ok": true,
                                "types": crate::entity_creation::TYPES,
                                "aliases": { "Polyline": "LwPolyline" },
                            }),
                        };
                        self.agent.reply_to_tool(server_request_id, result, None);
                        return Task::none();
                    }
                    "validate" => {
                        let request = parameters.get("request").cloned().unwrap_or(Value::Null);
                        let result = crate::mcp::preflight_request(&request);
                        self.agent.reply_to_tool(server_request_id, result, None);
                        return Task::none();
                    }
                    "tools" => {
                        self.agent.reply_to_tool(
                            server_request_id,
                            json!({ "ok": true, "status": "completed", "tools": embedded_tool_definitions(), "instructions": embedded_instructions() }),
                            None,
                        );
                        return Task::none();
                    }
                    _ => {}
                }
                if !self.agent_session_matches(&arguments) {
                    self.agent.reply_to_tool(
                        server_request_id,
                        json!({ "ok": false, "code": "session_changed", "error": "Read the active OCS session before continuing." }),
                        None,
                    );
                    return Task::none();
                }
                if op == "batch_result" {
                    let request_id = parameters
                        .get("request_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let result = self.agent.batch_results.get(request_id).cloned().unwrap_or_else(|| {
                        json!({ "ok": false, "status": "failed", "code": "unknown_batch", "error": "No retained batch result exists in this agent session." })
                    });
                    self.agent.reply_to_tool(server_request_id, result, None);
                    return Task::none();
                }
                match op {
                    "state" | "hello" => {
                        let state = self
                            .control_request(json!({
                                "op": op,
                                "protocol": 1,
                                "session_id": arguments["ocs_session_id"],
                            }))
                            .0;
                        if state["ok"].as_bool() == Some(true) {
                            self.agent.document_context = Some(state.clone());
                        }
                        self.agent.reply_to_tool(server_request_id, state, None);
                        return Task::none();
                    }
                    _ => {
                        let mut request = parameters.into_iter().collect::<serde_json::Map<_, _>>();
                        request.insert("op".to_string(), Value::String(op.to_string()));
                        request.insert("protocol".to_string(), json!(1));
                        request.insert(
                            "session_id".to_string(),
                            arguments["ocs_session_id"].clone(),
                        );
                        Value::Object(request)
                    }
                }
            }
            "ocs_execute" => {
                if !self.agent_session_matches(&arguments) {
                    self.agent.reply_to_tool(
                        server_request_id,
                        json!({ "ok": false, "code": "session_changed", "error": "Read the active OCS session before continuing." }),
                        None,
                    );
                    return Task::none();
                }
                let Some(mut request) = arguments["request"].as_object().cloned() else {
                    self.agent.reply_to_tool(
                        server_request_id,
                        json!({ "ok": false, "code": "invalid_request", "error": "Missing request object" }),
                        None,
                    );
                    return Task::none();
                };
                let op = request
                    .get("op")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if let Err(error) =
                    crate::mcp_ops::validate_execute_request(&Value::Object(request.clone()), &op)
                {
                    self.agent.reply_to_tool(
                        server_request_id,
                        json!({ "ok": false, "code": "validation_failed", "error": error }),
                        None,
                    );
                    return Task::none();
                }
                request
                    .entry("request_id".to_string())
                    .or_insert_with(|| Value::String(format!("codex-{call_id}")));
                request.insert("protocol".to_string(), json!(1));
                request.insert(
                    "session_id".to_string(),
                    arguments["ocs_session_id"].clone(),
                );
                request.insert("client_id".to_string(), json!("embedded-codex"));
                if !matches!(
                    op.as_str(),
                    "new" | "open" | "stop" | "user_select" | "getpoint"
                ) {
                    let Some(context) = self.agent.document_context.clone() else {
                        self.agent.reply_to_tool(
                            server_request_id,
                            json!({ "ok": false, "code": "state_required", "error": "Read the active drawing state before editing." }),
                            None,
                        );
                        return Task::none();
                    };
                    // Switching/activating changes which document owns the
                    // active state, so the previous document's context must
                    // not be attached to that request.
                    if let Err(code) = attach_agent_document_context(&mut request, &op, &context) {
                        let error = match code {
                            "document_changed" => {
                                "The request targets a different document than the active state. Read state again before continuing."
                            }
                            _ => "Could not attach the active drawing context.",
                        };
                        self.agent.reply_to_tool(
                            server_request_id,
                            json!({ "ok": false, "code": code, "error": error }),
                            None,
                        );
                        return Task::none();
                    }
                }
                Value::Object(request)
            }
            "ocs_capture" => {
                if !self.agent_session_matches(&arguments) {
                    self.agent.reply_to_tool(
                        server_request_id,
                        json!({ "ok": false, "code": "session_changed", "error": "Read the active OCS session before continuing." }),
                        None,
                    );
                    return Task::none();
                }
                let path = std::env::temp_dir().join(format!(
                    "opencadstudio-agent-capture-{}-{}.png",
                    std::process::id(),
                    call_id.replace(|ch: char| !ch.is_ascii_alphanumeric(), "_")
                ));
                capture_max_dimension = arguments["max_dimension"]
                    .as_u64()
                    .unwrap_or(1600)
                    .clamp(256, 4096) as u32;
                capture_path = Some(path.clone());
                let mut request = serde_json::Map::new();
                request.insert("op".to_string(), json!("capture"));
                request.insert("protocol".to_string(), json!(1));
                request.insert(
                    "request_id".to_string(),
                    json!(arguments
                        .get("request_id")
                        .and_then(Value::as_str)
                        .unwrap_or_else(|| call_id.as_str())),
                );
                request.insert(
                    "session_id".to_string(),
                    arguments["ocs_session_id"].clone(),
                );
                request.insert("client_id".to_string(), json!("embedded-codex"));
                let Some(context) = self.agent.document_context.clone() else {
                    self.agent.reply_to_tool(
                        server_request_id,
                        json!({ "ok": false, "code": "state_required", "error": "Read the active drawing state before capturing it." }),
                        None,
                    );
                    return Task::none();
                };
                for key in [
                    "document_id",
                    "revision",
                    "geometry_revision",
                    "camera_revision",
                ] {
                    request.insert(key.to_string(), context[key].clone());
                }
                if let Some(scope) = arguments.get("scope") {
                    request.insert("scope".to_string(), scope.clone());
                }
                request.insert("max_dimension".to_string(), json!(capture_max_dimension));
                if let Some(annotate) = arguments.get("annotate") {
                    request.insert("annotate".to_string(), annotate.clone());
                }
                request.insert(
                    "path".to_string(),
                    json!(path.to_string_lossy().to_string()),
                );
                for key in ["view", "bounds", "focus_handles", "highlight_handles"] {
                    if let Some(value) = arguments.get(key) {
                        request.insert(key.to_string(), value.clone());
                    }
                }
                Value::Object(request)
            }
            _ => {
                self.agent.reply_to_tool(
                    server_request_id,
                    json!({ "ok": false, "code": "unknown_tool", "error": format!("Unknown OpenCADStudio tool: {tool}") }),
                    None,
                );
                return Task::none();
            }
        };

        let operation_id = request["request_id"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| {
                let serial = self.agent.next_operation;
                self.agent.next_operation += 1;
                format!("codex-operation-{serial}")
            });
        let batch_result_id = (request["op"] == "batch").then(|| operation_id.clone());
        let (result, task) = self.control_request(request);
        if let Some(state) = result.get("state") {
            if state["ok"].as_bool() == Some(true) {
                self.agent.document_context = Some(state.clone());
            }
        }
        if matches!(result["status"].as_str(), Some("accepted" | "running")) {
            self.agent.pending_tools.push(PendingToolCall {
                server_request_id,
                operation_id,
                capture_path,
                capture_max_dimension,
                batch_result_id,
            });
        } else {
            if let Some(batch_id) = batch_result_id {
                self.agent.retain_batch_result(batch_id, result.clone());
            }
            let image = capture_path.and_then(|path| {
                let image = agent_capture_data_url(&path, capture_max_dimension);
                let _ = std::fs::remove_file(path);
                image
            });
            self.agent.reply_to_tool(server_request_id, result, image);
        }
        task
    }

    fn agent_session_matches(&self, arguments: &Value) -> bool {
        arguments["ocs_session_id"]
            .as_str()
            .is_some_and(|session| session == super::control::session_id())
    }

    pub(super) fn poll_agent_tools(&mut self) -> Task<Message> {
        let pending = std::mem::take(&mut self.agent.pending_tools);
        for call in pending {
            let result = self
                .control_request(json!({ "op": "operation", "request_id": call.operation_id }))
                .0;
            if matches!(result["status"].as_str(), Some("running" | "accepted")) {
                self.agent.pending_tools.push(call);
                continue;
            }
            if let Some(batch_id) = call.batch_result_id.as_ref() {
                self.agent
                    .retain_batch_result(batch_id.clone(), result.clone());
            }
            let state = self.control_request(json!({ "op": "state" })).0;
            if state["ok"].as_bool() == Some(true) {
                self.agent.document_context = Some(state);
            }
            let image = call.capture_path.and_then(|path| {
                let image = agent_capture_data_url(&path, call.capture_max_dimension);
                let _ = std::fs::remove_file(path);
                image
            });
            self.agent
                .reply_to_tool(call.server_request_id, result, image);
        }
        Task::none()
    }
}

fn agent_capture_data_url(path: &std::path::Path, max_dimension: u32) -> Option<String> {
    use image::ImageFormat;
    use std::io::Cursor;

    let bytes = std::fs::read(path).ok()?;
    let image = image::load_from_memory(&bytes).ok()?;
    let scale = (max_dimension as f64 / image.width().max(image.height()) as f64).min(1.0);
    let width = ((image.width() as f64 * scale).round() as u32).max(1);
    let height = ((image.height() as f64 * scale).round() as u32).max(1);
    let image = image.resize(width, height, image::imageops::FilterType::Triangle);
    let mut png = Cursor::new(Vec::new());
    image.write_to(&mut png, ImageFormat::Png).ok()?;
    use base64::Engine as _;
    Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png.into_inner())
    ))
}

fn bounded_transcript_value(value: &Value, max_chars: usize) -> String {
    let text = value.to_string();
    bounded_transcript_text(&text, max_chars)
}

fn bounded_transcript_text(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let bounded: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{bounded}…")
    } else {
        bounded
    }
}

fn attach_agent_document_context(
    request: &mut serde_json::Map<String, Value>,
    op: &str,
    context: &Value,
) -> Result<(), &'static str> {
    if matches!(op, "activate" | "switch_document") {
        return Ok(());
    }

    if request
        .get("document_id")
        .is_some_and(|document_id| document_id != &context["document_id"])
    {
        return Err("document_changed");
    }

    request
        .entry("document_id".to_string())
        .or_insert_with(|| context["document_id"].clone());
    for key in ["revision", "geometry_revision", "camera_revision"] {
        request
            .entry(key.to_string())
            .or_insert_with(|| context[key].clone());
    }

    Ok(())
}

fn dynamic_tools() -> Vec<DynamicToolSpec> {
    embedded_tool_definitions()
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tool| {
            Some(DynamicToolSpec::Function(DynamicToolFunctionSpec {
                name: tool.get("name")?.as_str()?.to_string(),
                description: tool.get("description")?.as_str()?.to_string(),
                input_schema: tool.get("inputSchema")?.clone(),
                defer_loading: false,
            }))
        })
        .collect()
}

/// Reuse OCS's MCP schemas while replacing descriptions that assume the
/// separate MCP bridge or launching another editor. The underlying tool
/// contract stays the same as the public MCP surface.
fn embedded_tool_definitions() -> Value {
    let mut tools = crate::mcp::tool_definitions();
    let Some(tools) = tools.as_array_mut() else {
        return Value::Array(Vec::new());
    };
    for tool in tools.iter_mut() {
        let name = tool["name"].as_str().unwrap_or_default().to_owned();
        match name.as_str() {
            "ocs_sessions" => {
                tool["description"] = json!(
                    "Read the OpenCADStudio instance that contains this agent and list its open documents. This agent operates on that same in-process drawing workspace."
                );
                if let Some(properties) = tool["inputSchema"]["properties"].as_object_mut() {
                    properties.remove("launch_if_none");
                }
            }
            "ocs_read" => {
                tool["description"] = json!(
                    "Read active drawing state, capabilities, command manifests, database records, entities, properties, layers, measurements, spatial relationships, history, and operation status. Native recognize reads live model-space structure with source handles, topology and review items; use layers and tolerance and follow independent collection page cursors. Use entity_schema for creation fields, validate for a preflight, and guide for CAD workflow information. Use batch_result for retained batch details from this session."
                );
            }
            "ocs_execute" => {
                tool["description"] = json!(
                    "Execute OpenCADStudio commands, semantic drawing edits, and batch operations in the active drawing. Read the architecture guide before floorplan vectorization and verify edits from their results."
                );
            }
            "ocs_capture" => {
                tool["description"] = json!(
                    "Capture the current OpenCADStudio drawing viewport as a bounded PNG with optional camera framing, entity highlighting, spatial coordinates, and visual annotations."
                );
                let supported = [
                    "ocs_session_id",
                    "request_id",
                    "scope",
                    "max_dimension",
                    "view",
                    "bounds",
                    "focus_handles",
                    "highlight_handles",
                    "annotate",
                ];
                if let Some(properties) = tool["inputSchema"]["properties"].as_object_mut() {
                    properties.retain(|name, _| supported.contains(&name.as_str()));
                }
                tool["inputSchema"]["additionalProperties"] = json!(false);
            }
            _ => {}
        }
    }
    Value::Array(tools.clone())
}

fn embedded_instructions() -> &'static str {
    "You are the native CAD drawing and visual-inspection agent inside OpenCADStudio. Operate on the active in-process drawing only through the supplied OpenCADStudio tools. Start with ocs_sessions, then read the active document state using ocs_read and preserve its session, document, and revision context. For arbitrary database inspection, use ocs_read op records without a collection to discover available collections, then record_schema to discover fields, then records with collection, handles, where and paths. These operations read the active in-memory CadDocument, including unsaved edits; they do not need a file path, export, Python or another CAD application. Follow next_offset until null and retain source handles. Read entities, objects, block_records, layers, styles, header and metadata as needed. For equipment, inspect Insert attributes and follow block_record entity_handles into block definitions; query geometry is a summary and records returns complete serialized properties. Do not equate every block with equipment or a bounded recognition pass with a complete equipment count. Parser-unsupported data cannot be recovered by a tool that only reads parsed records. Call these supplied native tools directly, without a JavaScript exec wrapper. For structure recognition, call ocs_read op recognize first with the relevant centerline layers and a suitable drawing-unit tolerance; it reads the live CAD database natively, returns source handles and bounded topology, and places unsupported or ambiguous cases in reviewItems. Follow page cursors and complete; supported-pass completion does not mean complete domain understanding. Use viewport capture when visual context matters; distinguish world coordinates from pixels and state uncertainty. Discover registered CAD commands using ocs_read op commands before running one through ocs_execute op run; follow state.command.accepts, options, and input_example while a command waits for input. Use entity_schema and validate before structured creation or edits. Make changes only when requested, wait for completion, and verify them by reading the resulting drawing or capturing the viewport. After a timeout, query the existing operation and never replay a mutation with a new request_id. Native recognize does not start Python or export DXF. A domain plugin command is available only when the command manifest advertises it. Read workflow guides on demand with ocs_read op guide."
}

async fn run_agent(
    mut commands: mpsc::UnboundedReceiver<AgentCommand>,
    events: mpsc::UnboundedSender<AgentEvent>,
) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut client: InProcessAppServerClient = match start_embedded_surface_client(
        cwd,
        "opencadstudio-native".to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
    )
    .await
    {
        Ok(client) => client,
        Err(error) => {
            let _ = events.unbounded_send(AgentEvent::Error(format!(
                "Could not initialize Codex: {error}"
            )));
            return;
        }
    };
    let request_handle = client.request_handle();
    let _ = events.unbounded_send(AgentEvent::Status(
        "Embedded Codex runtime connected".to_string(),
    ));
    let mut thread_id: Option<String> = None;
    let mut turn_id: Option<String> = None;
    let mut request_serial = 1_u64;

    loop {
        tokio::select! {
            command = commands.next() => {
                match command {
                    Some(command @ (AgentCommand::Account(_) | AgentCommand::LoginApiKey(_) | AgentCommand::CancelLogin(_))) => {
                        handle_account_command(&client, command, &mut request_serial, &events).await;
                    }
                    Some(AgentCommand::Prompt { text, cwd, model, effort }) => {
                        if thread_id.is_none() {
                            let params = ThreadStartParams {
                                cwd: Some(cwd.to_string_lossy().to_string()),
                                model: model.clone(),
                                approval_policy: Some(AskForApproval::OnRequest),
                                sandbox: Some(SandboxMode::ReadOnly),
                                dynamic_tools: Some(dynamic_tools()),
                                developer_instructions: Some(embedded_instructions().to_string()),
                                ..ThreadStartParams::default()
                            };
                            let started = client.request_typed::<ThreadStartResponse>(ClientRequest::ThreadStart {
                                request_id: next_request_id(&mut request_serial),
                                params,
                            }).await;
                            match started {
                                Ok(response) => {
                                    let id = response.thread.id;
                                    thread_id = Some(id.clone());
                                    let _ = events.unbounded_send(AgentEvent::ConversationStarted { id });
                                },
                                Err(error) => {
                                    let _ = events.unbounded_send(AgentEvent::Error(format!("Could not start the CAD agent thread: {error}")));
                                    continue;
                                }
                            }
                        }
                        let Some(thread) = thread_id.clone() else { continue; };
                        let params = serde_json::from_value::<TurnStartParams>(json!({
                            "threadId": thread,
                            "cwd": cwd,
                            "model": model,
                            "effort": effort,
                            "input": [{ "type": "text", "text": text, "textElements": [] }]
                        }));
                        let params = match params {
                            Ok(params) => params,
                            Err(error) => {
                                let _ = events.unbounded_send(AgentEvent::Error(format!("Invalid Codex turn settings: {error}")));
                                continue;
                            }
                        };
                        let turn = client.request_typed::<serde_json::Value>(ClientRequest::TurnStart {
                            request_id: next_request_id(&mut request_serial),
                            params,
                        }).await;
                        match turn {
                            Ok(response) => {
                                turn_id = response.get("turn").and_then(|turn| turn.get("id"))
                                    .and_then(Value::as_str).map(str::to_string);
                                let _ = events.unbounded_send(AgentEvent::Status("Codex is working on the drawing…".to_string()));
                            }
                            Err(error) => {
                                let _ = events.unbounded_send(AgentEvent::Error(format!("Could not start the Codex turn: {error}")));
                            }
                        }
                    }
                    Some(AgentCommand::ListConversations) => {
                        let params = serde_json::from_value::<ThreadListParams>(json!({
                            "limit": 20,
                            "sortKey": "updated_at",
                            "sortDirection": "desc",
                            "sourceKinds": [{ "custom": "opencadstudio" }],
                            "useStateDbOnly": true
                        }));
                        match params {
                            Ok(params) => match client.request_typed::<ThreadListResponse>(ClientRequest::ThreadList {
                                request_id: next_request_id(&mut request_serial),
                                params,
                            }).await {
                                Ok(response) => {
                                    let conversations = response.data.into_iter().map(|thread| AgentConversation {
                                        id: thread.id,
                                        title: thread.name.unwrap_or_default(),
                                        preview: thread.preview,
                                    }).collect();
                                    let _ = events.unbounded_send(AgentEvent::ConversationList(conversations));
                                }
                                Err(error) => {
                                    let _ = events.unbounded_send(AgentEvent::Error(format!("Could not load conversations: {error}")));
                                }
                            },
                            Err(error) => {
                                let _ = events.unbounded_send(AgentEvent::Error(format!("Invalid conversation query: {error}")));
                            }
                        }
                    }
                    Some(AgentCommand::LoadModels) => {
                        let params = serde_json::from_value::<ModelListParams>(json!({
                            "limit": 100,
                            "includeHidden": false
                        }));
                        match params {
                            Ok(params) => match client.request_typed::<ModelListResponse>(ClientRequest::ModelList {
                                request_id: next_request_id(&mut request_serial),
                                params,
                            }).await {
                                Ok(response) => {
                                    let models = response.data.into_iter()
                                        .map(|model| (model.model, model.display_name))
                                        .collect();
                                    let _ = events.unbounded_send(AgentEvent::ModelCatalog { models });
                                }
                                Err(error) => {
                                    let _ = events.unbounded_send(AgentEvent::Error(format!("Could not load Codex models: {error}")));
                                }
                            },
                            Err(error) => {
                                let _ = events.unbounded_send(AgentEvent::Error(format!("Invalid model query: {error}")));
                            }
                        }
                    }
                    Some(AgentCommand::ResumeConversation { id, cwd }) => {
                        let params = serde_json::from_value::<ThreadResumeParams>(json!({
                            "threadId": id,
                            "cwd": cwd.to_string_lossy().to_string(),
                            "excludeTurns": true,
                            "initialTurnsPage": {
                                "limit": 30,
                                "sortDirection": "desc",
                                "itemsView": "full"
                            }
                        }));
                        match params {
                            Ok(params) => match client.request_typed::<ThreadResumeResponse>(ClientRequest::ThreadResume {
                                request_id: next_request_id(&mut request_serial),
                                params,
                            }).await {
                                Ok(response) => {
                                    let title = response.thread.name.clone()
                                        .unwrap_or_else(|| response.thread.preview.clone());
                                    let messages = thread_messages_from_resume(&response);
                                    let id = response.thread.id;
                                    thread_id = Some(id.clone());
                                    turn_id = None;
                                    let _ = events.unbounded_send(AgentEvent::ConversationLoaded { id, title, messages });
                                }
                                Err(error) => {
                                    let _ = events.unbounded_send(AgentEvent::Error(format!("Could not resume conversation: {error}")));
                                }
                            },
                            Err(error) => {
                                let _ = events.unbounded_send(AgentEvent::Error(format!("Invalid conversation request: {error}")));
                            }
                        }
                    }
                    Some(AgentCommand::NewConversation) => {
                        thread_id = None;
                        turn_id = None;
                    }
                    Some(AgentCommand::ResolveRequest { request_id, result }) => {
                        match serde_json::from_value::<RequestId>(request_id) {
                            Ok(request_id) => {
                                if let Err(error) = request_handle.resolve_server_request(request_id, result).await {
                                    let _ = events.unbounded_send(AgentEvent::Error(format!("Could not return the CAD agent response: {error}")));
                                }
                            }
                            Err(error) => {
                                let _ = events.unbounded_send(AgentEvent::Error(format!("Invalid Codex tool request id: {error}")));
                            }
                        }
                    }
                    Some(AgentCommand::Stop) => {
                        if let (Some(thread_id), Some(turn_id)) = (thread_id.clone(), turn_id.clone()) {
                            let _ = client.request_typed::<serde_json::Value>(ClientRequest::TurnInterrupt {
                                request_id: next_request_id(&mut request_serial),
                                params: TurnInterruptParams { thread_id, turn_id },
                            }).await;
                        }
                    }
                    None => break,
                }
            }
            event = client.next_event() => {
                match event.map(AppServerEvent::from) {
                    Some(AppServerEvent::ServerNotification(notification)) => match *notification {
                        ServerNotification::AgentMessageDelta(delta) => {
                            if active_agent_turn(&thread_id, &turn_id, &delta.thread_id, &delta.turn_id) {
                                let _ = events.unbounded_send(AgentEvent::AssistantDelta(delta.delta));
                            }
                        }
                        ServerNotification::TurnPlanUpdated(plan) => {
                            if active_agent_turn(&thread_id, &turn_id, &plan.thread_id, &plan.turn_id) {
                                let _ = events.unbounded_send(AgentEvent::Plan(plan));
                            }
                        }
                        ServerNotification::TurnCompleted(completed) => {
                            if active_agent_turn(&thread_id, &turn_id, &completed.thread_id, &completed.turn.id) {
                                turn_id = None;
                                if let Some(error) = completed.turn.error {
                                    let _ = events.unbounded_send(AgentEvent::Error(error.message));
                                }
                                let _ = events.unbounded_send(AgentEvent::TurnComplete);
                            }
                        }
                        ServerNotification::Error(error) => {
                            if active_agent_turn(&thread_id, &turn_id, &error.thread_id, &error.turn_id) {
                                let event = if error.will_retry {
                                    AgentEvent::TurnRetrying(error.error.message)
                                } else {
                                    AgentEvent::Error(error.error.message)
                                };
                                let _ = events.unbounded_send(event);
                            }
                        }
                        ServerNotification::AccountLoginCompleted(completed) => {
                            let _ = events.unbounded_send(AgentEvent::LoginCompleted {
                                id: completed.login_id,
                                success: completed.success,
                                error: completed.error,
                            });
                        }
                        ServerNotification::AccountUpdated(_) => {
                            read_agent_account(&client, &mut request_serial, &events).await;
                        }
                        _ => {}
                    },
                    Some(AppServerEvent::ServerRequest(request)) => {
                        match *request {
                            ServerRequest::ToolRequestUserInput { request_id, params } => {
                                if active_agent_turn(&thread_id, &turn_id, &params.thread_id, &params.turn_id) {
                                    let _ = events.unbounded_send(AgentEvent::Questions {
                                        request_id: serde_json::to_value(request_id).unwrap_or(Value::Null), params,
                                    });
                                } else {
                                    let _ = request_handle.resolve_server_request(request_id, json!({"answers": {}})).await;
                                }
                            }
                            ServerRequest::DynamicToolCall { request_id, params } => {
                                let _ = events.unbounded_send(AgentEvent::ToolCall {
                                    server_request_id: serde_json::to_value(request_id).unwrap_or(Value::Null),
                                    call_id: params.call_id,
                                    tool: params.tool,
                                    arguments: params.arguments,
                                });
                            }
                            request @ ServerRequest::CommandExecutionRequestApproval { .. }
                            | request @ ServerRequest::FileChangeRequestApproval { .. }
                            | request @ ServerRequest::PermissionsRequestApproval { .. } => {
                                if let Some(approval) = agent_approval_from_request(&request) {
                                    let _ = events.unbounded_send(AgentEvent::ApprovalRequest(approval));
                                }
                            }
                            other => {
                                let request_id = other.id().clone();
                                let _ = request_handle.reject_server_request(request_id, codex_app_server_protocol::JSONRPCErrorError {
                                    code: -32601,
                                    message: "OpenCADStudio does not support this Codex server request".to_string(),
                                    data: None,
                                }).await;
                            }
                        }
                    }
                    Some(AppServerEvent::Lagged { skipped }) => {
                        let _ = events.unbounded_send(AgentEvent::Status(format!("Codex event stream skipped {skipped} update(s)")));
                    }
                    Some(AppServerEvent::Disconnected { message }) => {
                        let _ = events.unbounded_send(AgentEvent::Error(format!("Codex disconnected: {message}")));
                        break;
                    }
                    None => break,
                }
            }
        }
    }
    let _ = client.shutdown().await;
}

fn agent_error_message(error: &str) -> String {
    let lower = error.to_ascii_lowercase();
    if lower.contains("only supports chat completions")
        || lower.contains("requires responses for tool calls")
    {
        return format!("模型服务协议不兼容：当前代理的上游账号仅支持 Chat Completions，CAD Agent 的工具调用需要 Responses API。请在模型代理中切换到支持 Responses 工具调用的上游账号或服务，然后重试。\n\n服务原始错误：{error}");
    }
    if lower.contains("401") || lower.contains("unauthorized") || lower.contains("not authenticated") {
        return format!("模型服务身份验证失败。请检查当前服务的登录状态或 API Key，然后重试。\n\n{error}");
    }
    error.to_string()
}

fn active_agent_turn(
    thread: &Option<String>,
    turn: &Option<String>,
    event_thread: &str,
    event_turn: &str,
) -> bool {
    thread.as_deref() == Some(event_thread) && turn.as_deref() == Some(event_turn)
}

async fn read_agent_account(
    client: &InProcessAppServerClient,
    serial: &mut u64,
    events: &mpsc::UnboundedSender<AgentEvent>,
) {
    let event = match client
        .request_typed::<GetAccountResponse>(ClientRequest::GetAccount {
            request_id: next_request_id(serial),
            params: GetAccountParams {
                refresh_token: false,
            },
        })
        .await
    {
        Ok(response) => AgentEvent::AccountLoaded(response),
        Err(error) => AgentEvent::AccountStatus(format!("Could not read account: {error}")),
    };
    let _ = events.unbounded_send(event);
}

async fn handle_account_command(
    client: &InProcessAppServerClient,
    command: AgentCommand,
    serial: &mut u64,
    events: &mpsc::UnboundedSender<AgentEvent>,
) {
    let login_params = match command {
        AgentCommand::Account(AgentAccountAction::Refresh) => {
            read_agent_account(client, serial, events).await;
            return;
        }
        AgentCommand::Account(AgentAccountAction::RefreshUsage) => {
            let event = match client
                .request_typed::<GetAccountRateLimitsResponse>(
                    ClientRequest::GetAccountRateLimits {
                        request_id: next_request_id(serial),
                        params: None,
                    },
                )
                .await
            {
                Ok(response) => AgentEvent::UsageLoaded(response),
                Err(error) => AgentEvent::AccountStatus(format!("Could not read usage: {error}")),
            };
            let _ = events.unbounded_send(event);
            return;
        }
        AgentCommand::Account(AgentAccountAction::Logout) => {
            match client
                .request_typed::<LogoutAccountResponse>(ClientRequest::LogoutAccount {
                    request_id: next_request_id(serial),
                    params: None,
                })
                .await
            {
                Ok(_) => {
                    let _ = events.unbounded_send(AgentEvent::AccountCleared);
                    read_agent_account(client, serial, events).await;
                }
                Err(error) => {
                    let _ = events.unbounded_send(AgentEvent::AccountStatus(format!(
                        "Could not sign out: {error}"
                    )));
                }
            }
            return;
        }
        AgentCommand::CancelLogin(id) => {
            match client
                .request_typed::<CancelLoginAccountResponse>(ClientRequest::CancelLoginAccount {
                    request_id: next_request_id(serial),
                    params: CancelLoginAccountParams {
                        login_id: id.clone(),
                    },
                })
                .await
            {
                Ok(_) => {
                    let _ = events.unbounded_send(AgentEvent::LoginCanceled(id));
                    read_agent_account(client, serial, events).await;
                }
                Err(error) => {
                    let _ = events.unbounded_send(AgentEvent::AccountStatus(format!(
                        "Could not cancel sign-in: {error}"
                    )));
                }
            }
            return;
        }
        AgentCommand::Account(AgentAccountAction::LoginDevice) => {
            LoginAccountParams::ChatgptDeviceCode
        }
        AgentCommand::Account(AgentAccountAction::LoginBrowser) => LoginAccountParams::Chatgpt {
            codex_streamlined_login: false,
            use_hosted_login_success_page: false,
            app_brand: None,
        },
        AgentCommand::LoginApiKey(key) => LoginAccountParams::ApiKey {
            api_key: key.0.trim().to_string(),
        },
        _ => return,
    };
    match client
        .request_typed::<LoginAccountResponse>(ClientRequest::LoginAccount {
            request_id: next_request_id(serial),
            params: login_params,
        })
        .await
    {
        Ok(LoginAccountResponse::Chatgpt { login_id, auth_url }) => {
            let _ = events.unbounded_send(AgentEvent::LoginStarted {
                id: login_id,
                url: auth_url,
                code: None,
            });
        }
        Ok(LoginAccountResponse::ChatgptDeviceCode {
            login_id,
            verification_url,
            user_code,
        }) => {
            let _ = events.unbounded_send(AgentEvent::LoginStarted {
                id: login_id,
                url: verification_url,
                code: Some(user_code),
            });
        }
        Ok(_) => read_agent_account(client, serial, events).await,
        Err(error) => {
            let _ = events.unbounded_send(AgentEvent::AccountStatus(format!(
                "Could not sign in: {error}"
            )));
        }
    }
}

fn thread_messages_from_resume(response: &ThreadResumeResponse) -> Vec<ChatMessage> {
    if let Some(page) = response.initial_turns_page.as_ref() {
        return thread_messages(&page.data, true);
    }
    thread_messages(&response.thread.turns, false)
}

fn thread_messages(
    turns: &[codex_app_server_protocol::Turn],
    newest_first: bool,
) -> Vec<ChatMessage> {
    let mut messages = Vec::new();
    let iter: Box<dyn Iterator<Item = _>> = if newest_first {
        Box::new(turns.iter().rev())
    } else {
        Box::new(turns.iter())
    };
    for turn in iter {
        for item in &turn.items {
            match item {
                ThreadItem::UserMessage { content, .. } => {
                    for input in content {
                        match input {
                            UserInput::Text { text, .. } if !text.trim().is_empty() => {
                                messages.push(ChatMessage::new("user", text.clone()));
                            }
                            UserInput::Image { .. } | UserInput::LocalImage { .. } => {
                                messages.push(ChatMessage::new("user", "[Drawing image]"));
                            }
                            _ => {}
                        }
                    }
                }
                ThreadItem::AgentMessage { text, .. } if !text.trim().is_empty() => {
                    messages.push(ChatMessage::new("assistant", text.clone()));
                }
                ThreadItem::DynamicToolCall {
                    namespace,
                    tool,
                    arguments,
                    status,
                    content_items,
                    success,
                    ..
                } => {
                    let name = namespace
                        .as_ref()
                        .filter(|namespace| !namespace.is_empty())
                        .map(|namespace| format!("{namespace}/{tool}"))
                        .unwrap_or_else(|| tool.clone());
                    messages.push(ChatMessage::new(
                        "tool",
                        format!("{name}\n{}", bounded_transcript_value(arguments, 1_200)),
                    ));
                    let label = match (status, success) {
                        (DynamicToolCallStatus::Failed, _) | (_, Some(false)) => "Failed",
                        (DynamicToolCallStatus::Completed, _) => "Completed",
                        (DynamicToolCallStatus::InProgress, _) => "In progress",
                    };
                    let mut result = label.to_string();
                    for item in content_items.iter().flatten() {
                        if result.chars().count() > 2_000 {
                            break;
                        }
                        let text = match item {
                            DynamicToolCallOutputContentItem::InputText { text } => text.as_str(),
                            DynamicToolCallOutputContentItem::InputImage { .. } => {
                                "[Drawing image]"
                            }
                            DynamicToolCallOutputContentItem::InputAudio { .. } => "[Audio]",
                        };
                        if !text.trim().is_empty() {
                            result.push('\n');
                            result.extend(text.chars().take(2_001 - result.chars().count()));
                        }
                    }
                    messages.push(ChatMessage::new(
                        "tool-result",
                        bounded_transcript_text(&result, 2_000),
                    ));
                }
                _ => {}
            }
        }
    }
    messages
}

fn agent_approval_from_request(request: &ServerRequest) -> Option<AgentApproval> {
    match request {
        ServerRequest::CommandExecutionRequestApproval { request_id, params } => {
            let approve = serde_json::to_value(CommandExecutionRequestApprovalResponse {
                decision: CommandExecutionApprovalDecision::Accept,
            })
            .ok()?;
            let reject = serde_json::to_value(CommandExecutionRequestApprovalResponse {
                decision: CommandExecutionApprovalDecision::Decline,
            })
            .ok()?;
            Some(AgentApproval {
                request_id: serde_json::to_value(request_id).ok()?,
                title: "Approve command execution?".to_string(),
                details: params.command.clone().unwrap_or_else(|| {
                    params
                        .reason
                        .clone()
                        .unwrap_or_else(|| "Codex requested a command".to_string())
                }),
                approve_response: approve,
                reject_response: reject,
            })
        }
        ServerRequest::FileChangeRequestApproval { request_id, params } => {
            let approve = serde_json::to_value(FileChangeRequestApprovalResponse {
                decision: FileChangeApprovalDecision::Accept,
            })
            .ok()?;
            let reject = serde_json::to_value(FileChangeRequestApprovalResponse {
                decision: FileChangeApprovalDecision::Decline,
            })
            .ok()?;
            Some(AgentApproval {
                request_id: serde_json::to_value(request_id).ok()?,
                title: "Approve file changes?".to_string(),
                details: params
                    .reason
                    .clone()
                    .or_else(|| {
                        params
                            .grant_root
                            .as_ref()
                            .map(|path| format!("Allow writes under {}", path.display()))
                    })
                    .unwrap_or_else(|| "Codex requested permission to change files".to_string()),
                approve_response: approve,
                reject_response: reject,
            })
        }
        ServerRequest::PermissionsRequestApproval { request_id, params } => {
            let approve = serde_json::to_value(PermissionsRequestApprovalResponse {
                permissions: codex_app_server_protocol::GrantedPermissionProfile {
                    network: params.permissions.network.clone(),
                    file_system: params.permissions.file_system.clone(),
                },
                scope: PermissionGrantScope::Turn,
                strict_auto_review: None,
            })
            .ok()?;
            let reject = serde_json::to_value(PermissionsRequestApprovalResponse {
                permissions: Default::default(),
                scope: PermissionGrantScope::Turn,
                strict_auto_review: None,
            })
            .ok()?;
            Some(AgentApproval {
                request_id: serde_json::to_value(request_id).ok()?,
                title: "Approve additional permissions?".to_string(),
                details: params.reason.clone().unwrap_or_else(|| {
                    serde_json::to_string(&params.permissions).unwrap_or_default()
                }),
                approve_response: approve,
                reject_response: reject,
            })
        }
        _ => None,
    }
}

fn next_request_id(serial: &mut u64) -> RequestId {
    let id = *serial;
    *serial = serial.saturating_add(1);
    RequestId::String(format!("opencadstudio-agent-{id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call_native_tool(app: &mut OpenCADStudio, tool: &str, arguments: Value) -> Value {
        let (sender, mut receiver) = mpsc::unbounded();
        app.agent.commands = Some(sender);
        let _ = app.execute_agent_tool(AgentEvent::ToolCall {
            server_request_id: json!("native-read-test"),
            call_id: "native-read-test".into(),
            tool: tool.into(),
            arguments,
        });
        let command = receiver.try_recv().expect("tool response channel");
        let AgentCommand::ResolveRequest { result, .. } = command else {
            panic!("expected dynamic tool response");
        };
        assert_eq!(result["success"], true, "{result}");
        serde_json::from_str(result["contentItems"][0]["text"].as_str().unwrap()).unwrap()
    }

    #[test]
    fn native_agent_reads_unsaved_attributes_and_follows_the_active_document() {
        use codec::entities::{AttributeEntity, Insert};
        use codec::types::Vector3;
        use codec::EntityType;

        let mut app = OpenCADStudio::new_for_test();
        app.automation_op(r#"{"op":"new"}"#);
        let mut insert = Insert::new("PUMP", Vector3::new(100.0, 100.0, 0.0));
        insert
            .attributes
            .push(AttributeEntity::simple("Name", "Pump A"));
        let handle = app.tabs[app.active_tab].scene.add_entity(EntityType::Insert(insert));
        let session = call_native_tool(&mut app, "ocs_sessions", json!({}));
        let session_id = session["result"][0]["session_id"].clone();
        let request = json!({
            "ocs_session_id":session_id, "op":"records",
            "parameters":{"collection":"entities", "handle":format!("{:X}", handle.value())}
        });
        let first = call_native_tool(&mut app, "ocs_read", request.clone());
        assert_eq!(first["count"], 1);
        assert_eq!(
            first["records"][0]["properties"]["attributes"][0]["value"],
            "Pump A"
        );
        assert_eq!(
            first["records"][0]["properties"]["insert_point"]["x"],
            100.0
        );
        // Nothing was saved or re-imported: a live database change must be
        // reflected by the next dynamic tool call.
        let EntityType::Insert(insert) = app.tabs[app.active_tab]
            .scene
            .document
            .get_entity_mut(handle)
            .unwrap()
        else {
            panic!("insert");
        };
        insert.attributes[0].value = "Pump B".into();
        let second = call_native_tool(&mut app, "ocs_read", request.clone());
        assert_eq!(
            second["records"][0]["properties"]["attributes"][0]["value"],
            "Pump B"
        );

        app.tabs.push(super::super::document::DocumentTab::new_drawing(99));
        app.active_tab = app.tabs.len() - 1;
        let next = call_native_tool(&mut app, "ocs_read", request);
        assert_ne!(next["document_id"], first["document_id"]);
        assert_eq!(next["count"], 0);
    }

    #[test]
    fn failed_turn_preserves_partial_reply_and_reason_after_completion() {
        let mut app = OpenCADStudio::new_for_test();
        app.agent.busy = true;
        app.agent.append_assistant_delta("Reading the drawing");
        let error = "gpt-6.1-sol requires Responses for tool calls; this account only supports Chat Completions";
        let _ = app.on_agent_event(Some(AgentEvent::Error(error.into())));
        let _ = app.on_agent_event(Some(AgentEvent::TurnComplete));
        assert!(!app.agent.busy);
        assert!(app.agent.status.contains("Responses"));
        assert!(app.agent.messages.iter().any(|m| m.text == "Reading the drawing"));
        assert!(app.agent.messages.last().unwrap().text.contains("模型服务协议不兼容"));
        assert!(!app.agent.messages.last().unwrap().text.contains("身份验证失败"));
    }

    #[test]
    fn retry_notification_does_not_end_the_active_turn() {
        let mut app = OpenCADStudio::new_for_test();
        app.agent.busy = true;
        app.agent.append_assistant_delta("Working");
        let _ = app.on_agent_event(Some(AgentEvent::TurnRetrying("temporary disconnect".into())));
        assert!(app.agent.busy);
        assert!(app.agent.streaming_message);
        assert!(app.agent.last_turn_error.is_none());
        assert_eq!(app.agent.messages[0].text, "Working");
    }

    #[test]
    fn catalog_keeps_runtime_model_selection() {
        let mut app = OpenCADStudio::new_for_test();
        let _ = app.on_agent_event(Some(AgentEvent::ModelCatalog {
            models: vec![("other-model".into(), "Other".into())],
        }));
        assert!(app.agent.selected_model().is_none());
        app.agent.model_input = "custom-model".into();
        let _ = app.on_agent_event(Some(AgentEvent::ModelCatalog { models: vec![] }));
        assert_eq!(app.agent.selected_model().as_deref(), Some("custom-model"));
    }

    fn question_request() -> ToolRequestUserInputParams {
        serde_json::from_value(json!({
            "threadId":"drawing-thread", "turnId":"turn-1", "itemId":"question-1",
            "questions":[
                {"id":"layer", "header":"Layer", "question":"Which layer?", "options":[{"label":"PIPES", "description":"Centerlines"}]},
                {"id":"tolerance", "header":"Tolerance", "question":"Endpoint tolerance?", "options":null}
            ]
        })).unwrap()
    }

    #[test]
    fn native_questions_require_explicit_answers_and_keep_question_ids() {
        let mut request = AgentQuestions {
            request_id: json!(1),
            params: question_request(),
            answers: vec![AgentSecret::default(); 2],
        };
        assert!(request.response(false).is_none());
        request.answers[0] = AgentSecret("PIPES".into());
        assert!(request.response(false).is_none());
        request.answers[1] = AgentSecret("0.5".into());
        let response = request.response(false).unwrap();
        assert_eq!(response.answers["layer"].answers, ["PIPES"]);
        assert_eq!(response.answers["tolerance"].answers, ["0.5"]);
        assert!(request
            .response(true)
            .unwrap()
            .answers
            .values()
            .all(|answer| answer.answers.is_empty()));
    }

    #[test]
    fn native_question_events_preserve_answers_and_clear_on_turn_completion() {
        let mut app = OpenCADStudio::new_for_test();
        for _ in 0..2 {
            let _ = app.on_agent_event(Some(AgentEvent::Questions {
                request_id: json!(1),
                params: question_request(),
            }));
            let _ = app.agent_question_input(json!(1), 0, AgentSecret("PIPES".into()));
        }
        assert_eq!(app.agent.questions.len(), 1);
        assert_eq!(app.agent.questions[0].answers[0].0, "PIPES");
        let _ = app.submit_agent_answers(json!(1), false);
        assert_eq!(app.agent.questions.len(), 1);
        let _ = app.on_agent_event(Some(AgentEvent::TurnComplete));
        assert!(app.agent.questions.is_empty());
    }

    #[test]
    fn api_key_input_is_redacted_in_message_debug_output() {
        let message = Message::AgentApiKeyInput(AgentSecret("test-private-key".into()));
        let debug = format!("{message:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("test-private-key"));
    }

    #[test]
    fn account_errors_do_not_end_a_turn_or_enter_the_transcript() {
        let mut app = OpenCADStudio::new_for_test();
        app.agent.busy = true;
        app.agent.account_pending = true;
        app.agent.append_assistant_delta("Reading the drawing");

        let _ = app.on_agent_event(Some(AgentEvent::AccountStatus("Usage unavailable".into())));

        assert!(app.agent.busy);
        assert!(app.agent.streaming_message);
        assert!(!app.agent.account_pending);
        assert_eq!(app.agent.messages.len(), 1);
        assert_eq!(app.agent.messages[0].text, "Reading the drawing");
    }

    #[test]
    fn stale_login_completion_preserves_the_current_login() {
        let mut app = OpenCADStudio::new_for_test();
        app.agent.login = Some(AgentLogin {
            id: "new-login".into(),
            url: "https://example.com".into(),
            code: None,
        });
        let _ = app.on_agent_event(Some(AgentEvent::LoginCompleted {
            id: Some("old-login".into()),
            success: false,
            error: Some("Canceled".into()),
        }));
        assert_eq!(app.agent.login.as_ref().unwrap().id, "new-login");

        let _ = app.on_agent_event(Some(AgentEvent::LoginCompleted {
            id: Some("new-login".into()),
            success: true,
            error: None,
        }));
        assert!(app.agent.login.is_none());
        assert_eq!(app.agent.account_status, "Signed in");
    }

    #[test]
    fn event_polling_stays_active_when_idle_without_duplicate_waiters() {
        let mut app = OpenCADStudio::new_for_test();
        let (_sender, receiver) = mpsc::unbounded();
        app.agent.events = Some(Arc::new(Mutex::new(receiver)));
        let _ = app.on_agent_event(Some(AgentEvent::Status("Ready".into())));
        assert!(!app.agent.busy);
        assert!(app.agent.polling);
        let _ = app.agent.poll_task();
        assert!(app.agent.polling);
        let _ = app.on_agent_event(None);
        assert!(!app.agent.polling);
        assert!(app.agent.events.is_none());
    }

    #[test]
    fn assistant_stream_after_tool_output_keeps_transcript_roles_separate() {
        let mut panel = AgentPanelState::default();
        panel.append_assistant_delta("Inspecting");
        panel.messages.push(ChatMessage::new("tool", "ocs_read"));
        panel
            .messages
            .push(ChatMessage::new("tool-result", "revision 12"));

        panel.append_assistant_delta("Found ");
        panel.append_assistant_delta("a circle");

        assert_eq!(panel.messages.len(), 4);
        assert_eq!(panel.messages[0].text, "Inspecting");
        assert_eq!(panel.messages[2].text, "revision 12");
        assert_eq!(panel.messages[3].role, "assistant");
        assert_eq!(panel.messages[3].text, "Found a circle");
    }

    #[test]
    fn transcript_supports_selection_but_rejects_editing() {
        let mut panel = AgentPanelState::default();
        panel.messages.push(ChatMessage::new("assistant", "copy me"));

        panel.apply_transcript_action(0, text_editor::Action::SelectAll);
        assert_eq!(panel.messages[0].content.selection().as_deref(), Some("copy me"));

        panel.apply_transcript_action(
            0,
            text_editor::Action::Edit(text_editor::Edit::Insert('!')),
        );
        assert_eq!(panel.messages[0].text, "copy me");
        assert_eq!(panel.messages[0].content.text(), "copy me");
    }

    #[test]
    fn transcript_tool_values_are_bounded_without_splitting_unicode() {
        assert_eq!(bounded_transcript_value(&json!("图纸文件"), 2), "\"图…");
        assert_eq!(bounded_transcript_value(&json!("CAD"), 8), "\"CAD\"");
    }

    #[test]
    fn native_startup_replaces_the_welcome_tab_with_one_drawing() {
        let mut app = OpenCADStudio::new_for_test();

        app.enter_drawing_workspace();

        assert_eq!(app.tabs.len(), 1);
        assert!(!app.tabs[0].is_start);
        assert_eq!(app.active_tab, 0);
    }

    #[test]
    fn embedded_tools_only_advertise_native_supported_capture_options() {
        let tools = embedded_tool_definitions();
        let tools = tools.as_array().expect("tool definitions");
        let names: Vec<_> = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert_eq!(
            names,
            ["ocs_sessions", "ocs_read", "ocs_execute", "ocs_capture"]
        );

        let sessions = tools
            .iter()
            .find(|tool| tool["name"] == "ocs_sessions")
            .unwrap();
        assert!(sessions["inputSchema"]["properties"]
            .get("launch_if_none")
            .is_none());
        assert!(!sessions["description"].as_str().unwrap().contains("bridge"));

        let capture = tools
            .iter()
            .find(|tool| tool["name"] == "ocs_capture")
            .unwrap();
        let properties = capture["inputSchema"]["properties"]
            .as_object()
            .expect("capture input properties");
        for name in [
            "scope",
            "view",
            "bounds",
            "focus_handles",
            "highlight_handles",
            "annotate",
        ] {
            assert!(properties.contains_key(name), "capture must support {name}");
        }
        for name in ["diff", "diff_mode", "tile", "pyramid_manifest", "delivery"] {
            assert!(
                !properties.contains_key(name),
                "capture must not advertise unhandled {name}"
            );
        }
        assert_eq!(capture["inputSchema"]["additionalProperties"], false);
    }

    #[test]
    fn embedded_agent_instructions_prioritize_native_cad_inspection_and_verification() {
        let instructions = embedded_instructions();

        for required in [
            "ocs_sessions",
            "ocs_read op commands",
            "ocs_execute op run",
            "viewport capture",
            "drawing-unit tolerance",
            "world coordinates from pixels",
            "ocs_read op recognize",
            "reviewItems",
            "verify them by reading the resulting drawing",
        ] {
            assert!(instructions.contains(required), "missing: {required}");
        }
    }

    #[test]
    fn document_switch_does_not_inherit_previous_document_revisions() {
        let context = json!({
            "document_id": 10,
            "revision": 12,
            "geometry_revision": 7,
            "camera_revision": 3,
        });

        for op in ["activate", "switch_document"] {
            let mut request = serde_json::Map::from_iter([("document_id".to_string(), json!(20))]);
            attach_agent_document_context(&mut request, op, &context).expect("document switch");
            assert_eq!(request["document_id"], 20);
            for key in ["revision", "geometry_revision", "camera_revision"] {
                assert!(request.get(key).is_none(), "{op} inherited {key}");
            }
        }
    }

    #[test]
    fn edits_inherit_active_document_and_revision_context() {
        let context = json!({
            "document_id": 10,
            "revision": 12,
            "geometry_revision": 7,
            "camera_revision": 3,
        });
        let mut request = serde_json::Map::new();

        attach_agent_document_context(&mut request, "run", &context).expect("edit context");

        assert_eq!(request["document_id"], 10);
        assert_eq!(request["revision"], 12);
        assert_eq!(request["geometry_revision"], 7);
        assert_eq!(request["camera_revision"], 3);
    }

    #[test]
    fn resumed_turn_history_is_rendered_in_chronological_order() {
        let turns: Vec<codex_app_server_protocol::Turn> = serde_json::from_value(json!([
            {
                "id": "latest",
                "rootTurnId": null,
                "items": [
                    { "type": "userMessage", "id": "u2", "clientId": null, "content": [{ "type": "text", "text": "Next", "textElements": [] }] },
                    { "type": "agentMessage", "id": "a2", "text": "Done", "phase": null, "memoryCitation": null, "delivery": null, "questions": null }
                ],
                "itemsView": "full",
                "status": "completed",
                "error": null,
                "startedAt": null,
                "completedAt": null,
                "durationMs": null
            },
            {
                "id": "first",
                "rootTurnId": null,
                "items": [
                    { "type": "userMessage", "id": "u1", "clientId": null, "content": [{ "type": "text", "text": "Inspect this drawing", "textElements": [] }] },
                    { "type": "agentMessage", "id": "a1", "text": "I will inspect it", "phase": null, "memoryCitation": null, "delivery": null, "questions": null }
                ],
                "itemsView": "full",
                "status": "completed",
                "error": null,
                "startedAt": null,
                "completedAt": null,
                "durationMs": null
            }
        ]))
        .expect("turn history should decode");

        let messages = thread_messages(&turns, true);
        let texts: Vec<_> = messages
            .iter()
            .map(|message| message.text.as_str())
            .collect();
        assert_eq!(
            texts,
            ["Inspect this drawing", "I will inspect it", "Next", "Done"]
        );
    }

    #[test]
    fn agent_history_query_preserves_custom_surface_identity() {
        let params: ThreadListParams = serde_json::from_value(json!({
            "sourceKinds": [{ "custom": "opencadstudio" }],
            "limit": 20,
            "useStateDbOnly": true
        }))
        .expect("native history filter should decode");
        assert_eq!(
            params.source_kinds,
            Some(vec![codex_app_server_protocol::ThreadSourceKind::Custom(
                "opencadstudio".to_string()
            )])
        );
    }

    #[test]
    fn resumed_cad_tool_history_preserves_order_and_hides_media_payloads() {
        let turns = vec![history_turn(vec![
            json!({ "type": "userMessage", "id": "u1", "content": [{ "type": "text", "text": "Inspect", "textElements": [] }] }),
            json!({
                "type": "dynamicToolCall", "id": "t1", "namespace": "cad",
                "tool": "ocs_read", "arguments": { "op": "state" }, "status": "completed",
                "success": true, "contentItems": [
                    { "type": "inputText", "text": "Drawing revision 12" },
                    { "type": "inputImage", "imageUrl": "data:image/png;base64,secret-image-payload" },
                    { "type": "inputAudio", "audioUrl": "data:audio/wav;base64,secret-audio-payload" }
                ]
            }),
            json!({ "type": "agentMessage", "id": "a1", "text": "Inspected" }),
        ])];

        let messages = thread_messages(&turns, false);
        let roles: Vec<_> = messages.iter().map(|message| message.role).collect();
        assert_eq!(roles, ["user", "tool", "tool-result", "assistant"]);
        assert_eq!(messages[1].text, "cad/ocs_read\n{\"op\":\"state\"}");
        assert_eq!(
            messages[2].text,
            "Completed\nDrawing revision 12\n[Drawing image]\n[Audio]"
        );
        assert_eq!(messages[3].text, "Inspected");
    }

    #[test]
    fn resumed_tool_history_includes_failure_and_unfinished_status() {
        for (status, success, expected) in [
            ("failed", Value::Null, "Failed"),
            ("completed", json!(false), "Failed"),
            ("completed", json!(true), "Completed"),
            ("inProgress", Value::Null, "In progress"),
        ] {
            let turn = history_turn(vec![json!({
                "type": "dynamicToolCall", "id": "t1", "tool": "ocs_execute",
                "arguments": {}, "status": status, "success": success, "contentItems": null
            })]);
            let messages = thread_messages(&[turn], false);
            assert_eq!(messages.len(), 2);
            assert_eq!(messages[1].text, expected);
        }
    }

    #[test]
    fn resumed_tool_history_bounds_large_unicode_results() {
        let turn = history_turn(vec![json!({
            "type": "dynamicToolCall", "id": "t1", "tool": "ocs_read",
            "arguments": { "query": "图".repeat(2_000) }, "status": "completed", "success": true,
            "contentItems": [
                { "type": "inputText", "text": "图".repeat(10_000) },
                { "type": "inputText", "text": "must not be appended" }
            ]
        })]);
        let messages = thread_messages(&[turn], false);
        assert_eq!(messages[0].text.chars().count(), "ocs_read\n".len() + 1_201);
        assert_eq!(messages[1].text.chars().count(), 2_001);
        assert!(messages[1].text.starts_with("Completed\n图"));
        assert!(messages[1].text.ends_with('…'));
        assert!(!messages[1].text.contains("must not be appended"));
    }

    fn history_turn(items: Vec<Value>) -> codex_app_server_protocol::Turn {
        serde_json::from_value(json!({
            "id": "turn-1", "items": items, "itemsView": "full", "status": "completed",
            "error": null
        }))
        .expect("tool history should decode")
    }

    #[test]
    fn command_approval_responses_are_typed_and_fail_closed_by_default() {
        let request: ServerRequest = serde_json::from_value(json!({
            "method": "item/commandExecution/requestApproval",
            "id": "approval-1",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "item-1",
                "startedAtMs": 1,
                "command": "example-command",
                "cwd": "C:/drawings"
            }
        }))
        .expect("command approval request should decode");
        let approval = agent_approval_from_request(&request).expect("approval card");

        let approved: CommandExecutionRequestApprovalResponse =
            serde_json::from_value(approval.approve_response).expect("approved response");
        let rejected: CommandExecutionRequestApprovalResponse =
            serde_json::from_value(approval.reject_response).expect("rejected response");
        assert_eq!(approved.decision, CommandExecutionApprovalDecision::Accept);
        assert_eq!(rejected.decision, CommandExecutionApprovalDecision::Decline);
    }

    #[test]
    fn permission_approval_grants_only_requested_permissions() {
        let request: ServerRequest = serde_json::from_value(json!({
            "method": "item/permissions/requestApproval",
            "id": "approval-2",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "item-2",
                "startedAtMs": 1,
                "cwd": "C:/drawings",
                "reason": "Allow CAD file inspection",
                "permissions": {
                    "network": { "enabled": false },
                    "fileSystem": { "read": ["C:/drawings"], "write": [] }
                }
            }
        }))
        .expect("permission approval request should decode");
        let approval = agent_approval_from_request(&request).expect("approval card");

        let approved: PermissionsRequestApprovalResponse =
            serde_json::from_value(approval.approve_response).expect("approved response");
        let rejected: PermissionsRequestApprovalResponse =
            serde_json::from_value(approval.reject_response).expect("rejected response");
        assert_eq!(
            approved.permissions.network,
            Some(codex_app_server_protocol::AdditionalNetworkPermissions {
                enabled: Some(false),
            })
        );
        let approved = serde_json::to_value(approved).expect("approved permissions should encode");
        assert_eq!(approved["permissions"]["network"]["enabled"], false);
        assert_eq!(
            approved["permissions"]["fileSystem"]["read"][0],
            "C:/drawings"
        );
        assert!(rejected.permissions.network.is_none());
        assert!(rejected.permissions.file_system.is_none());
    }
}
