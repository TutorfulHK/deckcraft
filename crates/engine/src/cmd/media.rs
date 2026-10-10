//! Media playback (Playback tab, the media control bar, agents): play / pause / stop / seek are
//! requests the UI host carries out with its player ([`crate::UiRequest::Media`]); the host reports
//! back through [`Session::media_status`], which `media.info` returns with the probe.

use deckcraft_model::{MediaClip, ShapeId, ShapeKind};
use serde_json::{Value, json};

use super::*;
use crate::{EngineError, MediaStatus, Result, Session, UiRequest};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(noundo "media.play", "Play", ["Playback", "Preview"], Some("Alt+P"), "{id?, from?: ms}", has_doc, play),
        cmd!(noundo "media.pause", "Pause", ["Playback", "Preview"], None, "{id?}", has_doc, pause),
        cmd!(noundo "media.toggle", "Play/Pause", [], None, "{id?}", has_doc, toggle),
        cmd!(noundo "media.stop", "Stop", ["Playback", "Preview"], None, "{id?}", has_doc, stop),
        cmd!(noundo "media.seek", "Seek", [], None, "{id?, ms}", has_doc, seek),
        cmd!(
            query "media.info",
            "Media Info",
            [],
            None,
            "{id?} → {id, name, contentType, bytes, durationMs, video, width, height, probe: {container, audio?, video?}, options, status?}",
            has_doc,
            info
        ),
        cmd!(
            query "media.bytes",
            "Media Bytes",
            [],
            None,
            "{id: flat media id} → {data: base64, contentType, name, bytes}",
            always,
            media_bytes
        ),
        cmd!(
            "media.posterFrame",
            "Poster Frame",
            ["Video Format", "Adjust"],
            None,
            "{id?, ms?: frame time (default: current position)}",
            has_doc,
            poster_frame
        ),
    ]
}

/// The media shape a command acts on: `id` / `ids` / the selection, else the only media shape on
/// the current slide.
fn target(s: &Session, p: &Value, cmd: &str) -> Result<(ShapeId, MediaClip)> {
    let st = s.doc()?;
    let media_of = |id: ShapeId| st.shape(id).and_then(|sh| if let ShapeKind::Media(m) = &sh.kind { Some((id, m.clone())) } else { None });
    let ids = targets(s, p)?;
    if let Some(found) = ids.iter().find_map(|id| media_of(*id)) {
        return Ok(found);
    }
    if !ids.is_empty() && (p.get("id").is_some() || p.get("ids").is_some()) {
        return Err(bad(cmd, "not a media shape"));
    }
    let all: Vec<(ShapeId, MediaClip)> = st.shapes().iter().filter_map(|sh| media_of(sh.id)).collect();
    match all.len() {
        1 => Ok(all.into_iter().next().ok_or_else(|| bad(cmd, "no media"))?),
        0 => Err(bad(cmd, "no audio or video on this slide")),
        _ => Err(bad(cmd, "select an audio or video, or pass `id`")),
    }
}

fn request(s: &mut Session, p: &Value, cmd: &str, action: &str, ms: Option<u64>) -> Result<Value> {
    let (shape, _) = target(s, p, cmd)?;
    s.ui_requests.push(UiRequest::Media { action: action.into(), shape, ms });
    Ok(json!({"id": shape}))
}

fn ms_param(p: &Value, key: &str) -> Option<u64> {
    p.get(key).and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0).map(|v| v as u64)
}

fn play(s: &mut Session, p: &Value) -> Result<Value> {
    request(s, p, "media.play", "play", ms_param(p, "from"))
}
fn pause(s: &mut Session, p: &Value) -> Result<Value> {
    request(s, p, "media.pause", "pause", None)
}
fn toggle(s: &mut Session, p: &Value) -> Result<Value> {
    request(s, p, "media.toggle", "toggle", None)
}
fn stop(s: &mut Session, p: &Value) -> Result<Value> {
    request(s, p, "media.stop", "stop", None)
}
fn seek(s: &mut Session, p: &Value) -> Result<Value> {
    let ms = ms_param(p, "ms").ok_or_else(|| bad("media.seek", "missing `ms`"))?;
    request(s, p, "media.seek", "seek", Some(ms))
}

/// What a probe of media bytes found (the subset commands use), or why nothing could be read.
pub(crate) struct Probe {
    pub has_video: bool,
    pub has_audio: bool,
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
    pub playable: bool,
    pub decodable_video: bool,
    pub codecs: Vec<String>,
    pub json: Value,
}

/// Probe media bytes (container, codecs, duration, picture size).
#[cfg(feature = "media")]
pub(crate) fn probe(bytes: &std::sync::Arc<Vec<u8>>) -> std::result::Result<Probe, String> {
    let info = deckcraft_media::probe(bytes).map_err(|e| e.to_string())?;
    let (width, height) = info.video.as_ref().map(|v| (v.width, v.height)).unwrap_or((0, 0));
    Ok(Probe {
        has_video: info.video.is_some(),
        has_audio: info.audio.is_some(),
        duration_ms: info.duration_ms,
        width,
        height,
        playable: info.playable(),
        decodable_video: info.video.as_ref().is_some_and(|v| v.decodable),
        codecs: info.audio.iter().map(|a| a.codec.clone()).chain(info.video.iter().map(|v| v.codec.clone())).collect(),
        json: json!({
            "container": info.container,
            "durationMs": info.duration_ms,
            "playable": info.playable(),
            "audio": info.audio.as_ref().map(|a| json!({"codec": a.codec, "sampleRate": a.sample_rate, "channels": a.channels, "decodable": a.decodable})),
            "video": info.video.as_ref().map(|v| json!({"codec": v.codec, "width": v.width, "height": v.height, "fps": (v.fps * 1000.0).round() / 1000.0, "frames": v.frames, "decodable": v.decodable})),
        }),
    })
}

/// Built without the `media` feature: media is embedded and saved, but not probed or decoded.
#[cfg(not(feature = "media"))]
pub(crate) fn probe(_bytes: &std::sync::Arc<Vec<u8>>) -> std::result::Result<Probe, String> {
    Err("media decoding is not built in".into())
}

fn info(s: &mut Session, p: &Value) -> Result<Value> {
    let (id, m) = target(s, p, "media.info")?;
    let st = s.doc()?;
    let item = st.doc.media(m.media).ok_or_else(|| EngineError::Other("the media data is missing".into()))?;
    let probe = probe(&item.data).map(|i| i.json).unwrap_or_else(|e| json!({"error": e}));
    Ok(json!({
        "id": id,
        "name": item.name,
        "contentType": item.content_type,
        "bytes": item.data.len(),
        "video": m.video,
        "durationMs": m.duration_ms,
        "width": m.width,
        "height": m.height,
        "poster": m.poster.is_some(),
        "probe": probe,
        "options": {
            "autoplay": m.autoplay, "loop": m.loop_play, "rewind": m.rewind, "acrossSlides": m.play_across_slides,
            "hide": m.hide_while_not_playing, "fullScreen": m.full_screen, "volume": m.volume,
            "trimStart": m.trim_start_ms, "trimEnd": m.trim_end_ms, "fadeIn": m.fade_in_ms, "fadeOut": m.fade_out_ms,
        },
        "status": s.media_status.get(&id),
    }))
}

#[cfg(not(feature = "media"))]
pub(crate) fn poster_png(_bytes: &std::sync::Arc<Vec<u8>>, _ms: u64) -> Option<Vec<u8>> {
    None
}

/// A poster frame PNG for video bytes at `ms`.
#[cfg(feature = "media")]
pub(crate) fn poster_png(bytes: &std::sync::Arc<Vec<u8>>, ms: u64) -> Option<Vec<u8>> {
    match deckcraft_media::poster_png(bytes, ms as f64 / 1000.0) {
        Ok(png) => Some(png),
        Err(e) => {
            log::info!("no poster frame: {e}");
            None
        }
    }
}

fn poster_frame(s: &mut Session, p: &Value) -> Result<Value> {
    let (id, m) = target(s, p, "media.posterFrame")?;
    if !m.video {
        return Err(bad("media.posterFrame", "only videos have poster frames"));
    }
    let ms = ms_param(p, "ms").or_else(|| s.media_status.get(&id).map(|st: &MediaStatus| st.position_ms)).unwrap_or(0);
    let data = s.doc()?.doc.media(m.media).map(|i| i.data.clone()).ok_or_else(|| EngineError::Other("the media data is missing".into()))?;
    let png = poster_png(&data, ms).ok_or_else(|| EngineError::Other("no frame could be decoded there".into()))?;
    let name = format!("poster{id}.png");
    let poster = s.edit(|doc, _| Ok(doc.add_media(&name, "image/png", png)))?;
    edit_shapes(s, &json!({"id": id}), "media.posterFrame", |sh| {
        if let ShapeKind::Media(m) = &mut sh.kind {
            m.poster = Some(poster);
        }
        Ok(())
    })?;
    Ok(json!({"id": id, "ms": ms}))
}

fn media_bytes(s: &mut Session, p: &Value) -> Result<Value> {
    let id = match p.get("id") {
        Some(Value::String(v)) => v.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => return Err(bad("media.bytes", "missing `id`")),
    };
    let b = crate::flat::media_bytes(s, &id).ok_or_else(|| bad("media.bytes", format!("no media `{id}`")))?;
    Ok(json!({"data": base64_encode(&b.data), "contentType": b.content_type, "name": b.name, "bytes": b.data.len()}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav() -> Vec<u8> {
        let rate = 8000u32;
        let samples: Vec<i16> = (0..rate / 2).map(|i| ((i as f32 * 0.3).sin() * 3000.0) as i16).collect();
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut b = b"RIFF".to_vec();
        b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        for v in [
            16u32.to_le_bytes().to_vec(),
            1u16.to_le_bytes().to_vec(),
            1u16.to_le_bytes().to_vec(),
            rate.to_le_bytes().to_vec(),
            (rate * 2).to_le_bytes().to_vec(),
            2u16.to_le_bytes().to_vec(),
            16u16.to_le_bytes().to_vec(),
        ] {
            b.extend_from_slice(&v);
        }
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(data.len() as u32).to_le_bytes());
        b.extend_from_slice(&data);
        b
    }

    #[test]
    fn insert_audio_probes_and_play_requests_reach_the_host() {
        let mut s = Session::with_new();
        let _ = s.execute("slide.new", &json!({"layout": "blank"}));
        let r = s.execute("insert.audio", &json!({"name": "tone.wav", "data": base64_encode(&wav())})).expect("insert");
        if cfg!(feature = "media") {
            assert_eq!(r["durationMs"], 500);
            assert_eq!(r["playable"], true);
        } else {
            // Built without the codecs: embedded all the same, reported as not playable here.
            assert_eq!(r["playable"], false);
            assert!(r["warning"].as_str().is_some_and(|w| w.contains("not built in")), "{r}");
        }
        let id = r["id"].as_u64().expect("id");
        s.ui_requests.clear();
        s.execute("media.play", &json!({"from": 100})).expect("play");
        assert_eq!(s.ui_requests, vec![UiRequest::Media { action: "play".into(), shape: ShapeId(id as u32), ms: Some(100) }]);
        s.execute("media.seek", &json!({"id": id, "ms": 250})).expect("seek");
        assert!(s.execute("media.seek", &json!({"id": id})).is_err());
        // The host reports state; media.info returns it with the probe.
        s.media_status.insert(ShapeId(id as u32), MediaStatus { state: "playing".into(), position_ms: 250, duration_ms: 500, error: None });
        let i = s.execute("media.info", &json!({})).expect("info");
        if cfg!(feature = "media") {
            assert_eq!(i["probe"]["audio"]["sampleRate"], 8000);
            assert_eq!(i["probe"]["container"], "wav");
        } else {
            assert!(i["probe"]["error"].is_string(), "{i}");
        }
        assert_eq!(i["status"]["positionMs"], 250);
        assert_eq!(i["status"]["state"], "playing");
        assert!(s.execute("media.posterFrame", &json!({})).is_err(), "audio has no poster frame");
    }

    #[test]
    fn media_commands_need_media() {
        let mut s = Session::with_new();
        let _ = s.execute("slide.new", &json!({"layout": "blank"}));
        assert!(s.execute("media.play", &json!({})).is_err());
        let sh = s.execute("shape.insert", &json!({"preset": "rect", "rect": [0, 0, 10, 10]})).expect("shape");
        assert!(s.execute("media.play", &json!({"id": sh["id"]})).is_err());
    }

    #[test]
    fn wma_inserts_but_reports_unplayable() {
        let mut s = Session::with_new();
        let _ = s.execute("slide.new", &json!({"layout": "blank"}));
        // A minimal ASF header object with no children.
        let mut asf = vec![];
        asf.extend_from_slice(&[0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE, 0x6C]);
        asf.extend_from_slice(&30u64.to_le_bytes());
        asf.extend_from_slice(&[0; 6]);
        let r = s.execute("insert.audio", &json!({"name": "song.wma", "data": base64_encode(&asf)})).expect("insert");
        assert_eq!(r["playable"], false);
        assert!(r["warning"].as_str().is_some_and(|w| w.contains("can't be played")), "{r}");
    }
}
