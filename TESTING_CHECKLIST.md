# Testing Checklist - ThodTalk v0.3.2

## 🧪 Unit & Integration Tests

### Audio Module
- [ ] Device initialization doesn't hang
- [ ] Buffer sizes (16) prevent audio gaps
- [ ] Per-user muting actually blocks audio playback
- [ ] Volume levels clamp to 0.0-1.5 correctly
- [ ] Opus encoding/decoding doesn't error on valid frames

### Session & Networking
- [ ] Auto-reconnect fires after 15s, 30s, 60s
- [ ] last_server_code saves and loads correctly
- [ ] Per-user mute persists across disconnect
- [ ] Graceful shutdown doesn't crash server
- [ ] Ban list works (banned users can't rejoin)

### Storage & Persistence
- [ ] Profile saves with new fields (muted_users, etc)
- [ ] Images compress correctly (PNG/JPEG/WebP)
- [ ] History appends without corruption
- [ ] Storage handles disk full gracefully

### UI & UX
- [ ] Quick join button appears when last_server_code exists
- [ ] Image zoom with mouse wheel: 0.5x to 8x range
- [ ] Context menu for users shows mute toggle
- [ ] FFmpeg settings visible in Sound settings
- [ ] All buttons centered and properly spaced

## 🎯 Manual Testing Scenarios

### Scenario 1: Fresh Start
1. [ ] Launch app first time
2. [ ] Create profile with avatar
3. [ ] Host server - get code
4. [ ] Join as guest - voice works
5. [ ] Send message and image
6. [ ] Close app cleanly

### Scenario 2: Audio Quality
1. [ ] Start voice channel
2. [ ] Speak clearly - no lag on speaker
3. [ ] Enable push-to-talk - no audio on release
4. [ ] Mute self - other person can't hear
5. [ ] Unmute - audio returns
6. [ ] Right-click person → mute voice
   - [ ] Other person still sees you in channel
   - [ ] Can't hear them anymore
   - [ ] Mute persists on disconnect

### Scenario 3: Reconnection
1. [ ] Join server (code: ABC123)
2. [ ] Network interrupt (disconnect WiFi)
3. [ ] Auto-reconnect engages at 15s
4. [ ] Successful reconnect
5. [ ] Last server code saved
6. [ ] Restart app
7. [ ] "Быстрое подключение" button visible
8. [ ] Click → rejoins ABC123

### Scenario 4: Admin Controls
1. [ ] Host opens server
2. [ ] 2-3 guests join
3. [ ] Admin mutes individual guest
   - [ ] All other guests still hear them
4. [ ] Admin kicks guest
   - [ ] Guest gets error: "Вы были исключены"
   - [ ] Guest can't rejoin
5. [ ] Admin closes server (toggle)
   - [ ] New guests can't join
   - [ ] Current members stay connected
6. [ ] Admin gracefully shuts down
   - [ ] All members disconnect with message
   - [ ] No error messages in anyone's UI

### Scenario 5: Images
1. [ ] Send small image (<1MB) - loads instantly
2. [ ] Send large image (10MB) - compresses
3. [ ] Click image - opens preview fullscreen
4. [ ] Scroll mouse wheel - zooms (0.5x to 8x)
5. [ ] Right-click image → Save
6. [ ] Click background → closes preview
7. [ ] Press ESC → closes preview
8. [ ] Zoom resets on new image

### Scenario 6: Settings
1. [ ] Change device → "Устройства сменятся при следующем входе"
2. [ ] Toggle sounds on/off → affects notification sounds
3. [ ] Toggle light theme → UI redraws
4. [ ] Set FFmpeg noise suppression toggle
5. [ ] Set voice enhancement toggle
6. [ ] Change push-to-talk key → works on new key
7. [ ] Save settings → persisted
8. [ ] Reload app → settings still there

### Scenario 7: Stress Test
1. [ ] 5+ people in voice channel
2. [ ] Everyone speaking at once
3. [ ] Sending images while speaking
4. [ ] Monitor: CPU <20%, RAM <100MB, no crashes
5. [ ] Mute 3 people - audio still flows for others

### Scenario 8: Error Handling
1. [ ] Invalid invite code → "Неверный код"
2. [ ] Network timeout → "Соединение прервано" + auto-retry
3. [ ] Server full → graceful rejection
4. [ ] Corrupt image file → proper error message
5. [ ] Microphone not found → helpful message
6. [ ] Disk full → saves history locally

## 📊 Performance Checks

### Audio Latency
- [ ] First audio frame: <150ms from speaking
- [ ] Continuous speaking: consistent <100ms E2E
- [ ] After mute toggle: <50ms to take effect
- [ ] Per-user mute: <1ms to filter

### Network
- [ ] Join LAN server: <2s
- [ ] Join internet server: <8s
- [ ] Send message: <100ms delivery
- [ ] Send image: depends on size (track in log)
- [ ] Auto-reconnect: starts at 15s mark ±2s

### Memory
- [ ] Idle: <40MB
- [ ] In voice call: <80MB
- [ ] With 5 people: <100MB
- [ ] No memory leaks after 1 hour

### CPU
- [ ] Idle: <1%
- [ ] In voice call (1 person): 5-8%
- [ ] In voice call (5 people): 10-15%
- [ ] Peak on join: <20% for 2s

## ✅ Final Checks

Before release:
- [ ] All files compile without warnings
- [ ] No panics in normal operation
- [ ] Error messages are helpful in Russian
- [ ] UI responsive (no 1s+ freezes)
- [ ] Settings persist across app restart
- [ ] History saves don't corrupt
- [ ] No hardcoded paths (Windows portable)
- [ ] Tests pass on fresh Windows install
- [ ] Works on Windows 10 & Windows 11

## 🐛 Known Issues & Workarounds

### Current Limitations
1. **FFmpeg integration**: Settings exist but not connected to audio processing yet
   - Workaround: Use Windows microphone settings for noise suppression
2. **Image compression**: Currently PNG only, no auto-compress for large files
   - Workaround: Pre-compress images before sending
3. **Ban list**: Resets when server restarts
   - Workaround: Don't restart server while mitigating abuse

## 📝 Regression Testing

Always test these after code changes:
1. Audio latency still <150ms
2. Auto-reconnect still works
3. Muting still blocks audio
4. Image preview zoom still 0.5-8x
5. Settings still persist
6. History still saves
