//! OmniRoute configuration, generation and review in Text ▸ AI.
use crate::FilmcraftApp;
use egui::{Rect, UiBuilder};
use serde_json::json;

#[derive(Clone, Default)]
struct Draft {
    config: filmcraft_engine::ai::Config,
    key: String,
    prompt: String,
    language: String,
    item: Option<u64>,
    status: serde_json::Value,
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let id = egui::Id::new("omniroute-draft");
    let mut draft = ui.ctx().data_mut(|d| d.get_temp::<Draft>(id)).unwrap_or_else(|| Draft {
        config: app.session.prefs.ai.clone(),
        language: "auto".into(),
        ..Default::default()
    });
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect.shrink(8.0)).id_salt("omniroute"));
    let mut actions = Vec::new();
    egui::ScrollArea::vertical().show(&mut child, |ui| {
        ui.heading(tl!("AI assistant"));
        ui.label(tl!("Audio and prompts are sent to your configured AI provider."));
        let connection = ui.collapsing(tl!("OmniRoute connection"), |ui| {
            for (label, name, value) in [
                (tl!("Gateway URL"), "url", &mut draft.config.base_url),
                (tl!("Text model"), "chatModel", &mut draft.config.chat_model),
                (tl!("Speech model"), "speechModel", &mut draft.config.speech_model),
            ] {
                ui.label(label);
                let r = ui.add(egui::TextEdit::singleline(value).char_limit(2048).desired_width(f32::INFINITY));
                app.auto.add(&format!("text.ai.{name}"), r.rect, label);
            }
            ui.label(tl!("API key (this session only)"));
            let r = ui.add(egui::TextEdit::singleline(&mut draft.key).password(true).char_limit(4096));
            app.auto.add("text.ai.apiKey", r.rect, "API key");
            if button(app, ui, "configure", tl!("Save connection"), true) {
                let mut p = json!({"base_url":draft.config.base_url,"chat_model":draft.config.chat_model,"speech_model":draft.config.speech_model});
                if !draft.key.is_empty() {
                    p["apiKey"] = json!(draft.key);
                }
                actions.push(("ai.configure", p));
            }
            if button(app, ui, "clearKey", tl!("Forget API key"), true) {
                actions.push(("ai.configure", json!({"apiKey":""})));
            }
            if button(app, ui, "models", tl!("List available models"), true) {
                actions.push(("ai.models", json!({})));
            }
        });
        app.auto.add("text.ai.connection", connection.header_response.rect, "OmniRoute connection");
        ui.separator();
        ui.label(tl!("Caption source"));
        let label = draft
            .item
            .and_then(|i| app.session.project.item(filmcraft_project::ItemId(i)))
            .map(|i| i.name.clone())
            .unwrap_or_else(|| tl!("Selected media").into());
        let combo = egui::ComboBox::from_id_salt("ai-media").selected_text(label).show_ui(ui, |ui| {
            let r = ui.selectable_value(&mut draft.item, None, tl!("Selected media"));
            app.auto.add("text.ai.item.selection", r.rect, "Selected media");
            for item in app.session.project.items.values().filter(|i| matches!(i.kind, filmcraft_project::ItemKind::Media(_))) {
                let r = ui.selectable_value(&mut draft.item, Some(item.id.0), &item.name);
                app.auto.add(&format!("text.ai.item.{}", item.id.0), r.rect, &item.name);
            }
        });
        app.auto.add("text.ai.item", combo.response.rect, "Caption source");
        ui.label(tl!("Language (auto, pt, en…)"));
        let r = ui.add(egui::TextEdit::singleline(&mut draft.language).char_limit(32));
        app.auto.add("text.ai.language", r.rect, "Language");
        let running = draft.status["state"] == "running";
        let has_sequence = app.session.active_sequence().is_some();
        if button(app, ui, "captions", tl!("Generate captions"), has_sequence && !running) {
            let mut p = json!({"language":draft.language});
            if let Some(item) = draft.item {
                p["item"] = json!(item);
            }
            actions.push(("ai.captions", p));
        }
        ui.separator();
        ui.label(tl!("Describe your edit or animation"));
        let r = ui.add(egui::TextEdit::multiline(&mut draft.prompt).char_limit(8000).desired_rows(3).desired_width(f32::INFINITY));
        app.auto.add("text.ai.prompt", r.rect, "AI instruction");
        if button(app, ui, "decoupage", tl!("Suggest decoupage"), has_sequence && !running) {
            actions.push(("ai.decoupage", json!({"prompt":draft.prompt})));
        }
        if button(app, ui, "graphics", tl!("Create motion graphics"), has_sequence && !running) {
            actions.push(("ai.graphics", json!({"prompt":draft.prompt})));
        }
        ui.separator();
        // Read-only poll: safe in the UI frame, no network work here.
        if let Ok(status) = app.session.execute("ai.status", json!({})) {
            draft.status = status;
        }
        let state = draft.status["state"].as_str().unwrap_or("idle");
        let state_label = match state {
            "running" => tl!("Generating…"),
            "ready" => tl!("Ready to review"),
            "failed" => tl!("Generation failed"),
            "cancelled" => tl!("Cancelled"),
            "applied" => tl!("Applied"),
            _ => tl!("Ready"),
        };
        ui.label(state_label);
        if state == "running" {
            ui.spinner();
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
        }
        if let Some(e) = draft.status["error"].as_str() {
            ui.label(e);
        }
        if draft.status["stale"] == true {
            ui.label(tl!("The project changed. Generate a new result before applying."));
        }
        if let Some(plan) = draft.status.get("plan") {
            let review = ui.collapsing(tl!("Review result"), |ui| {
                if let Some(keep) = plan["keep"].as_array() {
                    let words = filmcraft_engine::transcript::sequence_words(&app.session);
                    for k in keep {
                        let from = k["from"].as_u64().and_then(|n| usize::try_from(n).ok());
                        let to = k["to"].as_u64().and_then(|n| usize::try_from(n).ok());
                        if let (Some(from), Some(to)) = (from, to)
                            && let Some(slice) = to.checked_add(1).and_then(|end| words.get(from..end))
                        {
                            ui.label(slice.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "));
                        }
                        ui.label(k["reason"].as_str().unwrap_or_default());
                    }
                } else if let Some(words) = plan.pointer("/transcript/words").and_then(serde_json::Value::as_array) {
                    ui.label(words.iter().filter_map(|w| w["text"].as_str()).collect::<Vec<_>>().join(" "));
                } else if let Some(layers) = plan["layers"].as_array() {
                    for layer in layers {
                        let kind = match layer["kind"].as_str() {
                            Some("text") => tl!("Text"),
                            Some("rectangle") => tl!("Rectangle"),
                            _ => tl!("Ellipse"),
                        };
                        ui.group(|ui| {
                            ui.label(format!("{kind}: {}", layer["text"].as_str().unwrap_or_default()));
                            ui.label(tlf!(
                                "Start: frame {start} · Duration: {duration} frames",
                                start = layer["start_frame"],
                                duration = layer["duration_frames"]
                            ));
                        });
                    }
                } else if let Some(models) = plan.pointer("/models/data").and_then(serde_json::Value::as_array) {
                    for model in models {
                        if let Some(id) = model["id"].as_str() {
                            ui.label(id);
                        }
                    }
                }
            });
            app.auto.add("text.ai.review", review.header_response.rect, "Review result");
        }
        let applicable = state == "ready" && draft.status["stale"] != true && draft.status["plan"]["kind"] != "models";
        if button(app, ui, "apply", tl!("Apply result (undoable)"), applicable) {
            actions.push(("ai.apply", json!({})));
        }
        if button(app, ui, "cancel", tl!("Cancel request"), matches!(state, "running" | "ready")) {
            actions.push(("ai.cancel", json!({})));
        }
    });
    for (command, params) in actions {
        match crate::menus::invoke(app, ui.ctx(), command, params) {
            Ok(_) => {
                if command == "ai.configure" {
                    draft.key.clear();
                }
            }
            Err(e) => app.ui.status = e,
        }
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, draft));
}

fn button(app: &mut FilmcraftApp, ui: &mut egui::Ui, id: &str, label: &str, enabled: bool) -> bool {
    let r = ui.add_enabled(enabled, egui::Button::new(label));
    app.auto.add(&format!("text.ai.{id}"), r.rect, label);
    r.clicked()
}
