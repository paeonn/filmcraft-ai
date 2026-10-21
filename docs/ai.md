# AI editing with OmniRoute

Text panel ▸ **AI** connects the desktop editor to an OpenAI-compatible gateway such as
OmniRoute. Set the gateway's `/v1` root (default `http://127.0.0.1:20128/v1`), a text model,
a speech model, and your gateway API key under **OmniRoute connection**, then **Save connection**.
**List available models** queries the gateway's `/models` catalogue; copy the desired model IDs
into the corresponding fields. Provider capability varies: a chat model is not a speech model.

The address and model choices are saved in application preferences. The API key stays in memory
for this session, never in project files, preferences or the command journal. Enter it again after
restarting. **Forget API key** clears it. Remote gateways require HTTPS; HTTP is accepted only
for localhost. Requests never follow redirects. Audio and prompts are sent through the gateway
to the configured provider; running a local gateway does not make cloud inference local.

## Workflows

- **Generate captions:** choose a media item (or select one in the Project panel), open a sequence
  using that media, and choose a language code such as `pt` or `auto`. Audio is decoded to mono
  16 kHz PCM WAV in five-minute chunks, with a four-hour media limit. The primary audio stream
  must have 1–8 channels and a sample rate up to 384 kHz. The provider must support
  `verbose_json` and word timestamps. Plain text or segment-only responses fail with an actionable
  error; the editor never invents word timings. Review the transcript, then **Apply result** to
  store it and generate a caption track for the active sequence's transcript. Existing transcripts
  elsewhere in the sequence also contribute to that new track. Recognition does not currently
  import speaker labels. Chunks do not share recognition context.
- **Suggest decoupage:** requires an existing sequence transcript (generated above, locally with
  Whisper, or imported). Describe which passages to keep. The model returns chronological,
  inclusive word ranges with reasons, shown for review. Applying removes the complementary timeline
  ranges on every unlocked track and closes the gaps, retaining the selected passages in order.
  All media and caption tracks must be unlocked; existing caption tracks must have sync lock on.
  This is dialogue-based editing, not visual scene understanding or automatic B-roll selection.
  Limits: 20,000 sequence words, 200 KB request context, 128 retained passages.
- **Create motion graphics:** describe titles, lower thirds or shapes. Review the proposed layers,
  then apply to create native editable text, rectangles and ellipses on video tracks. Position
  animates linearly between the proposed start and end points. Frames are absolute sequence frames.
  Limits: 32 layers, 4,000 bytes of text per layer, 2–10,000 frames per layer, final frame ≤1,000,000.
  This creates native graphics, not generated video, arbitrary code or Adobe templates.

Generation runs on a background worker, without changing the project. **Cancel request** discards
the result; an HTTP call already in flight may take up to its 120-second timeout to return. A second
request waits until that worker finishes. Generation failures, malformed output and worker panics
become errors. Applying is one undo step; any failure rolls back the whole application. Edits or a
sequence change after starting a request make its result stale: generate a fresh result to apply.

The web build reports network AI as unsupported; local Whisper remains an independent option.

## Commands (MCP, CLI and control channel)

All workflows use the same engine commands as the UI. Parameter names below are exact.

| Command | Parameters / result |
|---|---|
| `ai.configure` | `base_url`, `chat_model`, `speech_model`, `apiKey` (optional strings). Omitted key retains it; empty clears it; changing gateway clears an omitted key. No credentials in response. |
| `ai.models` | `{}`; starts catalogue request |
| `ai.captions` | `item` optional media ID, `language` optional code / `auto`; starts recognition |
| `ai.decoupage` | `prompt` required text; starts passage selection |
| `ai.graphics` | `prompt` required text; starts graphic design |
| `ai.status` | `{}`; `state`: idle / running / ready / failed / cancelled / applied; ready includes `plan`, `stale` |
| `ai.cancel` | `{}`; discards current request/result |
| `ai.apply` | `{}`; applies the current valid result exactly once |

Call a generation command, poll `ai.status` until ready or failed, review `plan`, then call
`ai.apply`. Automation IDs are `text.tab.AI` and `text.ai.*`: `url`, `chatModel`, `speechModel`,
`apiKey`, `connection`, `configure`, `clearKey`, `models`, `item`, `language`, `prompt`, `captions`,
`decoupage`, `graphics`, `review`, `apply`, `cancel`. Never include keys in checked-in CLI scripts
or screenshots.

## Validation

`cargo test -p filmcraft-engine ai:: --lib` uses deterministic transports and synthetic media to
exercise review, application, undo, stale results, hostile parameters, invalid model output and
worker failures. HTTP contract tests use a local synthetic server. No paid model, API key or cloud
service is needed for the tests. A live provider smoke test is still necessary for its model-specific
timestamp and structured-output capabilities.
