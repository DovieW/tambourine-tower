# Recordings and History

The small microphone button on Home starts a recording without a shortcut.
The options button lets you choose **Dictation** or **Meeting**; Kolboo remembers
the choice. Recordings started here stay in History instead of being pasted into
another app. F3 dictation keeps its normal insertion behavior.

Dictation uses your normal transcription and optional rewriting settings. Meeting
has its own model choice, supports pause/resume and never rewrites. Where supported,
Computer audio adds the system output to your microphone. Nothing is transcribed
until you press Stop. Audio is saved locally during capture so interrupted sessions
can be recovered from Recording options → Saved recordings.

Meeting models marked **Managed** use your Kolboo access; **Your key** uses the
provider key you configured. This choice is independent of Dictation. Recovery
retains the original recording's mode and model choice.

Click a History card to expand it. Play or seek through its waveform; opening a card
does not start playback. Copy is a separate button. The overflow menu contains rerun,
request logs and deletion. Rerun produces a new result without changing your notes.

**Open full view** is available for every recording. It has a larger transcript,
search, an editable title and an explicit Edit button. Corrections save automatically;
watch Saving/Saved or the error message. Restore original brings back the original
transcript. If a save fails, keep the app open and retry or copy your draft. Unsaved
changes cannot survive a process crash.

For simple meeting speaker labels, select **GPT-4o Transcribe · speaker labels** with
your existing OpenAI key, or when the model is available through managed access.
Long recordings are uploaded in parts after Stop. Speaker labels restart for each
part: two people labelled Speaker A in different parts may not be the same person.
Editing the transcript does not retime its original speaker metadata.

Saved audio, transcripts, corrections and waveform caches stay in the app's local
data directory until retained/deleted according to your settings. They are not
encrypted by Kolboo; use OS account protection and disk encryption. Transcription
sends audio only to the provider/path you have selected.
