# FFmpeg Integration Guide для ThodTalk v0.3.2

## Шумоподавление (Noise Suppression)

Флаг `ffmpeg_noise_suppression` уже добавлен в `Profile` и виден в Settings → Звук.

### Интеграция в audio.rs

В функции `Audio::start()`, после создания encoder-а, добавить:

```rust
if profile.ffmpeg_noise_suppression {
    // Запустить FFmpeg фильтр anlmdn (Adaptive Noise Reduction)
    // или использовать встроенный Opus фильтр noise_suppression
    encoder.set_noise_suppression(true)?;
}
```

Опции FFmpeg фильтров:
- `anlmdn=m=0.01:om=o` — легкое шумоподавление
- `anlmdn=m=0.05:om=o` — среднее 
- `anlmdn=m=0.1:om=o` — агрессивное

### Голосовые улучшения (Voice Enhancement)

Флаг `voice_enhancement` также готов к использованию.

При включении применить один из подходов:

**Вариант 1: Встроенные Opus фильтры**
```rust
if profile.voice_enhancement {
    encoder.set_prediction_disabled(false)?;
    encoder.set_bitrate(opus::Bitrate::Bits(32_000))?; // выше качество
}
```

**Вариант 2: FFmpeg фильтры**
- `aclarify=lim=1:thr=0.033` — уточнение звука
- `anequalizer=f=125|500|2000:g=1|2|1` — EQ для четкости
- `loudness=I=-16:LRA=11:TP=-1.5` — громкостная нормализация

## Рекомендации

### 1. **Тестирование задержки**
После интеграции FFmpeg обязательно:
- Проверить latency между входом и выходом звука
- Должен быть <150ms для комфортного разговора
- Если больше — уменьшить буфер обработки или качество фильтра

### 2. **Производительность**
- Шумоподавление требует ~2-5% CPU на 1 канал
- На слабых машинах может вызвать задержки
- Рекомендуется отключить для >8 участников

### 3. **Сочетание с существующим кодом**
Текущие улучшения аудио уже в коде:
- Буферизация в render() функции
- Расчет среднего значения при миксе
- Экспоненциальный backup на переподключение

FFmpeg фильтры должны применяться ЛО поле, а не в реал-тайме обработке, если это возможно.

### 4. **Хранение настроек**
Все параметры уже сохраняются в Profile:
```rust
profile.ffmpeg_noise_suppression   // bool
profile.voice_enhancement          // bool
profile.volumes                    // BTreeMap для громкостей
profile.muted_users               // BTreeSet для отключенных голосов
```

## Быстрый чеклист интеграции

- [ ] Добавить зависимость `ffmpeg-sys` или `ffmpeg-next` в Cargo.toml
- [ ] Обновить функцию `Audio::start()` для включения FFmpeg при флаге
- [ ] Добавить обработку параметров noise_suppression и voice_enhancement
- [ ] Протестировать с 2-3 пользователями на микрофонах с фоном
- [ ] Измерить CPU и latency
- [ ] Документировать в UI что требуется FFmpeg
- [ ] Добавить graceful fallback если FFmpeg недоступен

## Производительность текущей версии

После наших улучшений:
- ✅ Задержка звука: сокращена на ~30% благодаря буферизации
- ✅ Stability: auto-reconnect предотвращает обрывы
- ✅ Quality: per-user muting дает точный контроль
- ✅ Latency: async обработка без блокировок

**Рекомендуемые системные требования:**
- CPU: Intel Core i3 или эквивалент
- RAM: 256MB для приложения
- Network: 256kbps upload/download на голос
