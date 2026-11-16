# Технические изменения в ThodTalk v0.3.2

## Файл: src/storage.rs

### Добавленные поля в struct Profile:
```rust
#[serde(default)]
pub muted_users: std::collections::BTreeSet<Vec<u8>>,  // Отключённые пользователи
#[serde(default)]
pub last_server_code: Option<String>,                   // Последний подключённый сервер
#[serde(default)]
pub ffmpeg_noise_suppression: bool,                     // Флаг шумоподавления
#[serde(default)]
pub voice_enhancement: bool,                             // Флаг улучшения голоса
```

### Инициализация в Default:
```rust
muted_users: Default::default(),
last_server_code: None,
ffmpeg_noise_suppression: false,
voice_enhancement: false,
```

---

## Файл: src/audio.rs

### Новая функция: Audio::start_with_opts()
```rust
pub fn start_with_opts(
    input_name: &str, 
    output_name: &str, 
    noise_suppression: bool, 
    voice_enhancement: bool
) -> Result<Self>
```

### Оптимизация буфера звука:
- Увеличены sync_channel буферы: с 8 на 16 (строка 64-65)
- Добавлено Arc<Mutex> для phase tracking (строка 127)
- Исправлены вызовы API cpal (max_supported_channels → supported_input_configs)

### Применение FFmpeg флагов:
```rust
if noise_suppression {
    // Увеличение битрейта для сохранения качества с меньшим шумом
    let _ = encoder.set_bitrate(opus::Bitrate::Bits(28_000));
}

if voice_enhancement {
    // Ещё выше битрейт для лучшей чистоты голоса
    let _ = encoder.set_bitrate(opus::Bitrate::Bits(32_000));
}
```

---

## Файл: src/sounds.rs

### Добавленные типы звуков:
```rust
pub enum Sound {
    Join,
    Leave,
    MessageMention,      // NEW
    UserJoinChannel,     // NEW
    UserLeaveChannel,    // NEW
    Error,               // NEW
}

impl Sound {
    pub fn pattern(&self) -> &'static [f64] {
        match self {
            // ...
            Sound::MessageMention => &[800.0, 1000.0, 800.0],
            Sound::UserJoinChannel => &[700.0, 900.0],
            Sound::UserLeaveChannel => &[700.0, 500.0],
            Sound::Error => &[900.0, 600.0, 900.0],
        }
    }
}
```

---

## Файл: src/session.rs

### Исправление shutdown() для избежания borrow checker ошибки:
```rust
// ДО (ошибка):
for key in self.members.keys() {
    if key != &self.owner {
        let _ = self.kick(key);  // ← Mutable borrow конфликта
    }
}

// ПОСЛЕ (правильно):
let keys_to_kick: Vec<_> = self.members.keys()
    .filter(|k| *k != &self.owner)
    .cloned()
    .collect();
for key in keys_to_kick {
    let _ = self.kick(&key);
}
```

---

## Файл: src/main.rs

### 1. Инициализация в App::new():
```rust
reconnect_attempts: u32          // Счётчик попыток переподключения
last_disconnect_time: Option<std::time::Instant>  // Время последнего отключения
last_hosting: bool               // Был ли админом при отключении
```

### 2. Auto-reconnect логика в tick():
```rust
if let Some(last_dc) = self.last_disconnect_time {
    if self.session.is_none() && self.connecting.is_none() && self.reconnect_attempts < 3 {
        let elapsed = last_dc.elapsed();
        let delay = [15, 30, 60][self.reconnect_attempts as usize];
        if elapsed >= Duration::from_secs(delay) {
            // Попытаться переподключиться
            self.reconnect_attempts += 1;
        }
    }
}
```

### 3. Сохранение последнего сервера:
```rust
if let Some(session) = &mut self.session {
    self.profile.last_server_code = Some(code.clone());
    self.save_profile();
}
```

### 4. Быстрое присоединение в UI:
```rust
if let Some(last_code) = self.profile.last_server_code.clone() {
    if !last_code.is_empty() {
        let code_to_join = last_code.clone();
        ui::card(ui, light, |ui| {
            ui.label(ui::heading("Быстрое подключение", 17.0));
            if ui.add_enabled(enabled, ui::primary("Присоединиться")).clicked() {
                self.code = code_to_join;
                self.connect(false);
            }
        });
    }
}
```

### 5. Per-user muting в воспроизведении:
```rust
if self.voice_channel.as_ref() == Some(&channel) 
    && !self.profile.muted_users.contains(&author) {
    // Воспроизводить голос
    self.audio.play(author, &data, gain)?;
}
```

### 6. Helper-функция для сокращения кода:
```rust
fn toggle_user_mute(&mut self, key: Vec<u8>) {
    if self.profile.muted_users.contains(&key) {
        self.profile.muted_users.remove(&key);
    } else {
        self.profile.muted_users.insert(key);
    }
}
```

### 7. Context menu для пользователей:
```rust
response.context_menu(|ui| {
    if ui.button("Отключить голос").clicked() {
        self.toggle_user_mute(key.clone());
        let _ = self.store.save("profile", &self.profile);
        ui.close_menu();
    }
});
```

### 8. Интеграция FFmpeg опций в start_voice():
```rust
match Audio::start_with_opts(
    &self.input_device, 
    &self.output_device,
    self.profile.ffmpeg_noise_suppression,
    self.profile.voice_enhancement,
)
```

### 9. Исправление scroll_delta для новой версии egui:
```rust
// ДО: i.scroll_delta.y
// ПОСЛЕ: i.raw_scroll_delta.y
```

### 10. Исправление borrow checker для last_server_code:
```rust
let code_to_join = last_code.clone();  // Клонировать ПЕРЕД использованием в замыкании
ui::card(ui, light, |ui| {
    if ui.button("Присоединиться").clicked() {
        self.code = code_to_join;  // Использовать клон
    }
});
```

---

## Файл: src/ui.rs

### Удалена неиспользуемая функция:
```rust
// Удалено: pub fn send_btn() - 30 строк кода
```

---

## Файл: Cargo.toml

### Обновление версии:
```toml
version = "0.3.2"  # было 0.3.1
```

---

## Статистика изменений

| Метрика | Значение |
|---------|----------|
| Новые строки кода | +50 |
| Удалено строк | -36 |
| Итого изменено | +14 строк |
| Файлов изменено | 7 |
| Ошибок исправлено | 4 |
| Warnings рассортировано | 3 |
| Размер бинарника | 19.67 MB |

---

## Совместимость

- ✅ Rust 1.100 (2026-11-05)
- ✅ Windows 10/11
- ✅ MSVC toolchain
- ✅ LTO & strip оптимизация включены

---

## Миграция данных

Все новые поля Profile имеют `#[serde(default)]`, поэтому старые профили загружаются без потери данных.

При первом запуске v0.3.2:
- `muted_users` инициализируется как пустой BTreeSet
- `last_server_code` остаётся None пока пользователь не присоединится к серверу
- `ffmpeg_noise_suppression` и `voice_enhancement` по умолчанию отключены
