use super::*;

struct Fake(Value);
impl Backend for Fake {
    fn request(&self, _: Request, _: &AtomicBool) -> std::result::Result<Value, String> {
        Ok(self.0.clone())
    }
}
fn session() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}
fn wait(s: &mut Session) -> Value {
    for _ in 0..500 {
        let r = s.execute("ai.status", json!({})).unwrap();
        if r["state"] != "running" {
            return r;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("AI worker did not finish")
}
fn transcript(s: &mut Session) -> ItemId {
    let it = s.active_sequence().unwrap().audio_tracks[0].items[0].clone();
    let words: Vec<_> = (0..4)
        .map(|i| Word::new(format!("word{i}"), it.source_in + Tick::from_seconds_f64(f64::from(i)), it.source_in + Tick::from_seconds_f64(f64::from(i) + 0.5)))
        .collect();
    s.execute("transcript.set", json!({"item":it.item.0,"transcript":Transcript { words, ..Default::default() }})).unwrap();
    it.item
}
fn layer() -> Layer {
    Layer {
        kind: "text".into(),
        text: "Original title".into(),
        start_frame: 0,
        duration_frames: 48,
        position: [100.0, 200.0],
        end_position: [300.0, 200.0],
        size: [400.0, 100.0],
        font_size: 64.0,
    }
}

#[test]
fn configuration_keeps_credentials_out_of_preferences_and_journal() {
    let mut s = Session::default();
    s.execute("ai.configure", json!({"apiKey":"secret-value","chat_model":"test/model"})).unwrap();
    assert_eq!(s.ai.key, "secret-value");
    assert!(!serde_json::to_string(&s.prefs).unwrap().contains("secret-value"));
    assert!(!serde_json::to_string(&s.journal).unwrap().contains("secret-value"));
    assert!(!s.execute("ai.status", json!({})).unwrap().to_string().contains("secret-value"));
    s.execute("ai.configure", json!({"base_url":"https://gateway.example/v1"})).unwrap();
    assert!(s.ai.key.is_empty());
    for p in [
        json!({"apiKey":12}),
        json!({"apiKey":"bad\r\nheader"}),
        json!({"base_url":"file:///etc/passwd"}),
        json!({"base_url":"http://remote.example/v1"}),
        json!({"base_url":"https://user:secret@example.com/v1"}),
        json!({"base_url":"https://example.com/v1?key=secret"}),
        json!({"chat_model":"x\r\ninjection"}),
        json!({"unknown":true}),
    ] {
        assert!(s.execute("ai.configure", p).is_err());
    }
}

#[test]
fn graphics_round_trip_animates_and_undoes_in_one_step() {
    let mut s = session();
    let before = s.project.clone();
    s.ai.backend = Some(Arc::new(Fake(json!({"choices":[{"message":{"content":json!({"layers":[layer()]}).to_string()}}]}))));
    s.execute("ai.graphics", json!({"prompt":"A sliding title"})).unwrap();
    assert_eq!(wait(&mut s)["state"], "ready");
    assert!(Arc::ptr_eq(&before, &s.project), "generation is read-only");
    let r = s.execute("ai.apply", json!({})).unwrap();
    let clip = filmcraft_project::ClipId(r["clips"][0].as_u64().unwrap());
    let (_, it) = s.active_sequence().unwrap().find_item(clip).unwrap();
    assert_eq!(it.duration, Tick(s.sequence_rate().frame_duration().0 * 48));
    let e = it.effects.iter().find(|e| e.effect == "graphic_text").unwrap();
    let p = e.params.get("position").unwrap();
    assert_eq!(p.keyframes.len(), 2);
    assert!(s.execute("ai.apply", json!({})).is_err(), "no duplicate application");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(*s.project, *before);
}

#[test]
fn stale_cancelled_failed_and_panicking_results_cannot_change_project() {
    let mut s = session();
    s.ai.backend = Some(Arc::new(Fake(json!({"choices":[{"message":{"content":json!({"layers":[layer()]}).to_string()}}]}))));
    s.execute("ai.graphics", json!({"prompt":"title"})).unwrap();
    wait(&mut s);
    s.execute("graphics.newText", json!({"text":"manual edit"})).unwrap();
    let before = s.project.clone();
    assert!(s.execute("ai.apply", json!({})).is_err());
    assert!(Arc::ptr_eq(&before, &s.project));
    s.execute("ai.cancel", json!({})).unwrap();
    assert!(s.execute("ai.apply", json!({})).is_err());
    s.ai.backend = Some(Arc::new(Fake(json!({"choices":[{"message":{"content":"not JSON"}}]}))));
    s.execute("ai.graphics", json!({"prompt":"title"})).unwrap();
    assert_eq!(wait(&mut s)["state"], "failed");
    assert!(s.execute("ai.apply", json!({})).is_err());
    start(&mut s, |_| panic!("synthetic worker failure")).unwrap();
    assert_eq!(wait(&mut s)["state"], "failed");
    assert!(Arc::ptr_eq(&before, &s.project));
}

#[test]
fn decoupage_keeps_selected_words_and_undo_restores_everything() {
    let mut s = session();
    transcript(&mut s);
    let before = s.project.clone();
    s.ai.backend =
        Some(Arc::new(Fake(json!({"choices":[{"message":{"content":json!({"keep":[{"from":1,"to":2,"reason":"Relevant passage"}]}).to_string()}}]}))));
    s.execute("ai.decoupage", json!({"prompt":"Keep the middle passage"})).unwrap();
    assert_eq!(wait(&mut s)["state"], "ready");
    s.execute("ai.apply", json!({})).unwrap();
    let words = crate::transcript::sequence_words(&s);
    assert_eq!(words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), vec!["word1", "word2"]);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(*s.project, *before);
}

#[test]
fn caption_round_trip_is_reviewable_and_undoable() {
    let mut s = session();
    let clip = s.active_sequence().unwrap().audio_tracks[0].items[0].clone();
    let item = clip.item;
    let offset = clip.source_in.0 as f64 / TICKS_PER_SECOND as f64;
    s.execute("ai.configure", json!({"speech_model":"test/whisper"})).unwrap();
    s.ai.backend = Some(Arc::new(Fake(
        json!({"language":"pt","words":[{"word":"Olá","start":offset+0.2,"end":offset+0.5},{"word":"mundo","start":offset+0.6,"end":offset+0.9}]}),
    )));
    let before = s.project.clone();
    s.execute("ai.captions", json!({"item":item.0,"language":"pt"})).unwrap();
    let r = wait(&mut s);
    assert_eq!(r["state"], "ready", "{r}");
    assert!(Arc::ptr_eq(&before, &s.project));
    s.execute("ai.apply", json!({})).unwrap();
    assert!(!s.active_sequence().unwrap().caption_tracks.is_empty());
    assert_eq!(s.project.transcripts.get(&item).unwrap().words.len(), 2);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(*s.project, *before);
}

#[test]
fn hostile_model_output_is_bounded_and_never_panics() {
    for response in [
        json!({}),
        json!({"words":"bad"}),
        json!({"words":[{"word":"x","start":-1,"end":1}]}),
        json!({"words":[{"word":"x","start":0,"end":10000000}]}),
        json!({"words":[{"word":"x","start":1,"end":0}]}),
    ] {
        assert!(std::panic::catch_unwind(|| parse_words(&response, 16000, 0)).unwrap().is_err());
    }
    for (a, b) in [(1, 0), (0, usize::MAX), (usize::MAX, usize::MAX)] {
        assert!(validate_selections(&[Selection { from: a, to: b, reason: String::new() }], 4).is_err());
    }
    for value in [f64::NAN, f64::INFINITY, -1000000.0] {
        let mut l = layer();
        l.position[0] = value;
        assert!(validate_layers(&[l]).is_err());
    }
    let mut l = layer();
    l.start_frame = u64::MAX;
    assert!(validate_layers(&[l]).is_err());
    assert!(wav(&[f32::NAN]).is_err());
    assert!(wav(&[]).is_err());
}

#[test]
fn hostile_command_parameters_never_modify_the_project() {
    let mut s = session();
    let before = s.project.clone();
    for id in ["ai.configure", "ai.models", "ai.captions", "ai.decoupage", "ai.graphics", "ai.status", "ai.cancel", "ai.apply"] {
        for p in [Value::Null, json!([]), json!({"unexpected":true})] {
            assert!(s.execute(id, p).is_err(), "{id}");
            assert!(Arc::ptr_eq(&before, &s.project));
        }
    }
    for p in [json!({}), json!({"prompt":false}), json!({"prompt":""}), json!({"prompt":"x".repeat(8001)})] {
        assert!(s.execute("ai.graphics", p).is_err());
    }
    for p in [json!({"item":-1}), json!({"item":u64::MAX}), json!({"item":1,"language":"pt\r\ninjection"})] {
        assert!(s.execute("ai.captions", p).is_err());
    }
    for frames in [json!(-1), json!(0), json!(u64::MAX), json!(1.5), json!("48")] {
        assert!(s.execute("graphics.newText", json!({"text":"invalid duration","durationFrames":frames})).is_err());
    }
    assert!(Arc::ptr_eq(&before, &s.project));
}

#[test]
fn word_times_use_chunk_offsets_and_missing_timestamps_are_errors() {
    let response = json!({"words":[{"word":"test","start":0.25,"end":0.5}]});
    let w = parse_words(&response, 16000, 16000).unwrap();
    assert_eq!(w[0].start, Tick(TICKS_PER_SECOND * 5 / 4));
    assert_eq!(w[0].end, Tick(TICKS_PER_SECOND * 3 / 2));
    assert!(parse_words(&json!({"text":"untimed speech"}), 16000, 0).is_err());
}

#[test]
fn failed_apply_rolls_back_earlier_graphics() {
    let mut s = session();
    let before = s.project.clone();
    let mut bad = layer();
    bad.kind = "unsupported".into();
    // Validation rejects the whole plan before any layer is committed.
    let out = s.edit_group("AI", |s| apply_layers(s, &[layer(), bad]));
    assert!(out.is_err());
    assert_eq!(*s.project, *before);
}
