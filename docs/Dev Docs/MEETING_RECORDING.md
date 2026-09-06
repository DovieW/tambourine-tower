# Home and meeting recording

## Behavior

- Home has a compact single-row floating recorder with icon-only Record, Pause/Resume,
  Stop & transcribe, Cancel, and elapsed captured time. Its options popover holds
  a remembered Dictation/Meeting selector. Meeting options include a separate model
  picker and Computer audio. Record has an accessible label but no tooltip. Detailed
  errors and saved recordings open a separate dialog; the popover has no scrollbar.
- Home recordings save transcripts to History, never type or paste into another
  application. That output mode belongs to the Rust session and also applies
  when F3 stops a Home recording. Ordinary F3 dictation is unchanged.
- Pausing keeps capture devices open but excludes paused samples. The elapsed
  counter measures retained audio, not wall-clock time. Ordinary F3 sessions do
  not offer meeting pause controls.
- Stop assembles one transcript and one successful History/playback entry. Dictation
  applies optional rewriting once; Meeting bypasses rewriting, routing, clipboard
  context and automatic OCR regardless of profile/preset settings. Nothing is
  transcribed while Home capture is running. There are no live captions.
- Recording preferences use the non-secret `recording_preferences` settings key.
  The mode, explicit meeting provider/model, and managed/BYOK route are snapshotted in an owner-only
  `.options.json` file before capture and copied alongside the complete saved WAV.
  Recovery and reruns use this snapshot, not the current popup selection. Missing
  legacy metadata means Dictation; unreadable metadata fails closed. Length never
  determines recording mode.
- Meeting's model picker lists enabled managed models and configured user-key/local
  options separately. Its route does not depend on or change Dictation's route.
  An unavailable managed choice fails with audio retained, never silently uses a key.
- Final audio is normalized to mono 16 kHz. After Stop, uploads contain at most
  ten minutes (~19.2 MB), below the managed gateway's 25 MB request limit. Cuts
  prefer a quiet boundary in the last ten seconds; sample ranges have no gaps or
  overlap. This is not periodic live transcription or separate History sections.
- The 50 MiB dictation limit remains unchanged. Meeting transcription and History
  reruns use a separate four-hour normalized-WAV ceiling. Capture is still limited
  to four hours or 2 GiB raw, whichever comes first (high-rate/stereo inputs can
  reach the disk ceiling sooner). Final WAV preparation uses bounded raw blocks;
  the current pipeline still holds the final WAV and copies in memory, about
  440 MiB per copy at four hours. Physical multi-hour acceptance remains unverified.
- Providers may impose shorter duration/timeouts or quotas. A failed upload keeps
  the complete source and completed progress for retry. Automatic splitting does
  not bypass quotas, authentication, or managed model policy.
- Cancel during capture discards that capture. Cancel during transcription keeps
  the full recovery audio.

## Computer audio capabilities

Linux uses the system FFmpeg PulseAudio input adapter (`/usr/bin/ffmpeg`) and
`pactl`. PipeWire's PulseAudio compatibility server is supported. On Ubuntu,
install `ffmpeg` and `pulseaudio-utils`. A Homebrew FFmpeg build without PulseAudio
input support is insufficient.

Enabling the switch records the **system default microphone and default output
monitor**, mixed to mono 16 kHz. Microphone-only mode uses Kolboo's selected input.
The switch starts off and is locked during a recording. A failed capture startup
returns an error instead of silently recording microphone-only audio. Output
device changes during a recording require stopping and starting a new recording.

Windows and macOS still support the microphone pipeline, but computer-audio
capture is explicitly unavailable there until native adapters are implemented
and tested. No cross-platform computer-audio release claim is made.

## Recovery and privacy

Home recordings deliberately write raw audio into `meeting-recovery` under the
application data directory. This is additional local persistence even if normal
completed-recording retention is disabled; the recorder explains it before use.
No provider request is made until Stop & transcribe or the saved recording's
Transcribe action.

- Append-only audio is synced approximately once per second and at normal stop.
  An abrupt process crash can lose the unsynced tail; incomplete final frames are
  ignored. The audio journal retains samples beyond the ordinary memory ring.
- Journals use owner-only file/directory permissions on Unix. Audio is not
  encrypted on disk; OS account and disk encryption protect it.
- A recording is limited to four hours or 2 GiB of raw audio, whichever comes
  first. Storage/capture failure stops the session and exposes retained audio for
  recovery. Recovery files are not automatically purged on an age timer.
- Home lists interrupted recordings. Transcribe resumes completed upload results
  from owner-only `.transcripts` checkpoints in the recovery directory. They
  contain sensitive transcript text and optional speaker segments, are not encrypted by Kolboo, and are synced
  after each successful upload. A partial trailing checkpoint line is ignored.
  Cache keys bind the complete audio checksum, sample range, provider/model,
  language, transcription prompt, and Meeting's managed/BYOK choice. Changing these starts fresh uploads.
  A successful History row prevents resubmission after a crash before cleanup.
  Cancellation, provider errors, and history persistence errors retain the source.
- A crash after a provider finishes but before its checkpoint is synced can repeat
  that one upload; this is resumability, not exactly-once provider billing.
  Explicit History reruns of completed meetings are fresh attempts without a
  persistent partial-text cache. Failed original recovery remains resumable.
- Legacy section progress is ignored when preparing a full final transcription;
  existing section History rows are preserved, not silently deleted.
- Successful completion removes the raw journal, mode metadata, partial transcripts, and any legacy cursor. Discard
  removes the selected journal. Delete all recordings includes recovery journals and rejects
  deletion while capture/transcription is active. Completed recording WAVs use the
  existing recording store and its controls.
- Recovery is exclusive with new recording and other retry commands. The
  recovery cancellation token also covers final audio preparation.

## Meeting speaker labels

`gpt-4o-transcribe-diarize` uses the existing OpenAI BYOK adapter, or the existing
managed Edge path when authorized and enabled in the Edge catalog. No provider key
is provisioned by this feature and no model/default is automatically enabled.
Requests use `response_format=diarized_json` and `chunking_strategy=auto`, with no
prompt, timestamp-granularity or known-speaker-reference fields.

Each upload's text and speaker segments are checkpointed. The displayed document
contains simple speaker-labelled paragraphs and part boundaries. Speaker A in
different parts does **not** assert the same identity. Original STT text and segment
metadata remain separate from manual corrections; edited text is never falsely
realigned to timestamps. There is no speaker management or synchronized highlighting.

## History reader, playback, and corrections

- List queries return a bounded 320-character preview plus lightweight metadata;
  expanding one card loads its complete document on demand. Explicit Copy loads
  complete corrected text, never the preview. Existing filters/pagination remain.
- Cards expand rather than copy. Inline transcripts scroll after 300 px. Every card
  offers Open full view: title, fixed player, literal search with previous/next
  matches, Copy and explicit Edit mode, above one scrolling reading surface.
- One HTML media element belongs to the History view. It pauses on collapse,
  modal close, switching entries and navigation, preserves session positions, and
  never autoplays on expansion. Controls include waveform seeking, keyboard seek,
  ±10 seconds, elapsed/total time and speed.
- Rust streams PCM to generate at most 4096 min/max waveform pairs, caches them
  beside the WAV and validates the source fingerprint. WaveSurfer renders these
  precomputed peaks; the webview does not decode the complete meeting to draw it.
  Tauri asset playback permits individual canonical files within RecordingStore,
  supports range requests, and grants no recursive directory access. The old
  whole-file base64 fallback and playback timeout are removed.
- Corrections are stored in `history-edits/<hashed-entry-id>.json`, owned by
  HistoryStorage. Original History output remains unchanged. Writes are serialized,
  use synced private temporary files and same-directory replacement, and reject
  stale revisions. Saves debounce at 600 ms with a five-second maximum while typing.
  Closing flushes pending edits; failed saves leave the draft available for copying,
  retry or explicit conflict resolution. Unsaved drafts survive view navigation in
  memory, not a process crash. No transcript is put in browser local storage.
- Restore original restores text without resetting the revision; reruns create
  separate entries referencing the same recording. Search, previews, Copy, analysis
  and existing exports use corrected text. Retention/deletion remove correction
  sidecars and original metadata with their entry; audio deletion removes waveform
  caches and recording options. Delete-transcripts also clears speaker metadata
  and persists a revision barrier: delayed autosaves and stale correction sidecars
  cannot restore cleared text after a restart.

Focused tests use synthetic audio, mocked provider HTTP and a local DOM environment.
Visual acceptance should use user screenshots of a collapsed card, expanded card,
full reader and recorder popover. Production enablement, paid provider smoke tests
and release publication are separate rollout actions, not part of this change.

## Entitlement status correction

A freshly validated Active entitlement without an expiration date remains
Active during its seven-day validation window. A refresh failure enters Grace;
an explicitly Expired entitlement is not revived by cached timestamps. Community
operation remains available without managed access.

## Validation and remaining acceptance

The History/recording-mode redesign passes the desktop's full Rust test command
(829 passed, 12 ignored), frontend tests (664 passed, 57 skipped), typechecking,
lint, formatting, Knip, renderer production build and the local-Whisper compile
check. API Edge's full suite passes (153 tests), including mocked enabled/disabled
diarization routes. No real credentials or paid API calls are needed by these tests.
Native playback, visual acceptance, and physical recording of this redesign still
require an app restart and manual checks. No release or deployment was performed.

### Earlier recorder baseline checks (before the History redesign)

Deterministic tests cover session output ownership, pause state, recovery job
exclusivity/cancellation, journal data beyond the memory ring, explicit discard,
partial crash frames, complete final WAV preparation, post-stop upload assembly,
checkpoint resumption, cancellation, isolated size limits, and entitlement
status. Frontend tests cover the controls and invoke contracts.

The Linux FFmpeg adapter was exercised against an isolated PulseAudio null sink,
including pause/resume and shutdown, without recording a physical microphone or
contacting a provider. Source-app startup and rendering were checked on this
machine. Physical microphone/speaker combinations, multi-hour recordings, sudden
power loss, and Windows/macOS acceptance remain manual checks—not completed
release evidence. The published beta does not include these changes until a new
release is built and authorized.

On this Ubuntu desktop, the final native-window check showed missing painted
content until the process was launched with `WEBKIT_DISABLE_DMABUF_RENDERER=1`.
That workaround is set only on the running local test service, not globally or
in packaged platform defaults. The final window then rendered correctly. The
optional cold Clippy run was stopped to keep the handoff bounded; it is not
reported as a passing check.
