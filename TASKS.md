# Task Tracker

## In Progress
- [ ] Working on all tasks below

## Completed
- [x] Add a setting for default location of the widget (center, top-left, top-center, top-right, bottom-left, bottom-center, bottom-right)

## To Do

### Settings & UI
- [ ] Option to just add text to clipboard and not auto paste (dropdown: auto-type, clipboard, keystrokes)
- [ ] Use tabs instead of accordions for the settings page
- [ ] Option to paste as keystrokes instead of paste (combined with above)
- [ ] Remove the "Reset all hotkeys to their default values" text
- [ ] The "This shortcut is already used for the hold hotkey" error should go away after 5 seconds
- [ ] Logs page has massive loading indicator - not sized right

### Functionality
- [ ] Does the "connected" dot make sense now? There's no server - rename to "Ready" status
- [ ] "Thank you" keeps appearing in history - filter out common Whisper hallucinations for silent audio

---

## Implementation Notes

### Output Mode Options (transcription_output_mode setting)
- `auto_paste` - Current behavior: copy to clipboard, simulate Ctrl+V/Cmd+V, restore clipboard
- `clipboard` - Just copy to clipboard (no auto-paste)
- `keystrokes` - Type each character as individual keystrokes (slower but works in more apps)

### Connected Dot
The "Connected" dot was from the old Python server architecture. Now it should show:
- Green "Ready" when pipeline is idle and configured
- Yellow "Recording" when recording
- Blue "Processing" when transcribing
- No need for "Disconnected" state anymore

### "Thank you" Hallucinations
Whisper models hallucinate common phrases like "Thank you", "Thanks for watching", "Subscribe" etc. when given silent/near-silent audio. Solutions:
1. Check audio RMS level before transcribing - skip if too quiet
2. Filter known hallucination phrases from output
3. Use VAD to detect if any speech was present
