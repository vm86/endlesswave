# Перенос радио-демона `yamusic-dj` на Rust

Цель: заменить Python-демон из `yamusic-radio` (около 8 000 строк, сборка PyInstaller) на Rust-крейт внутри
`yamusic-radio-player`. Интерфейс (Vue) и нативный плеер (`audio.rs`) остаются как есть.

## Принятые решения

- **Лицензия плеера: AGPL-3.0-or-later** (сделано: `LICENSE`, `NOTICE`, `Cargo.toml`, `tauri.conf.json`,
  `package.json`, README). Причина: статическая линковка Essentia (AGPL-3.0). Версии до 0.1.0 остаются под MIT.
- **Тональность: FFI на Essentia** (`KeyExtractor`, профиль `edma`) через C-обёртку. Fallback на профиль
  Крумхансла (feature-флаг `essentia` выключен).
- **Громкость: крейт `ebur128`** (BS.1770), Essentia для неё не нужна.
- **Контракт HTTP API не меняется:** те же `/api/*`, `/stream`, `/stream.flac`, `/stream.ogg`. Python остаётся
  эталоном на время миграции.

- **YouTube: отложен** (этап 5.4 вне текущего объёма; бэкенд `youtube` в Rust-версии пока не появляется).
- **Код живёт в `crates/radio-core`** этого репозитория.

- **Кодирование: ffmpeg-подпроцессы остаются** (MP3, FLAC, Ogg-FLAC, декодирование). Кодировщик прячется за trait
  `Encoder`, чтобы позже заменить его локально (этап 8).

Открытых вопросов нет.

## Что переносим

| Python (`yamusic-radio`) | Строк | Что делает | Rust |
|---|---|---|---|
| `dj_mix.py` | 1614 | Анализ (BPM, тональность, громкость), планирование и DSP переходов, подбор следующего трека | `mix/` на `rustfft`/`realfft` |
| `dj.py` | 3293 | `Radio` (очередь, состояние), `mix()`, HTTP API, потоки, кодирование | `radio/`, `api/`, `stream/` |
| `spectrum.py` | 174 | 12 полос спектра, SSE | `spectrum.rs` |
| `cache.py` | 303 | SQLite: анализы, настройки, история | `cache.rs` (`rusqlite` уже есть) |
| `multi.py`, `backend.py` | 614 | Несколько бэкендов, паузы после сбоев | `backend/` (trait `MusicBackend`) |
| `local.py` | 280 | Папка с файлами, теги через ffmpeg | `backend/local.rs` |
| `ya_player.py`, `request.py` | 718 | Яндекс: OAuth, rotor, подпись загрузки, rate limit | `backend/yandex.rs` |
| `youtube.py` | 324 | `ytmusicapi` + `yt-dlp` | `backend/youtube.rs` |
| `main.py` | 420 | CLI-флаги, регистрация бэкендов | `config.rs`, запуск из `lib.rs` |

Тесты радио: около 6 000 строк (`test_dj.py` — 5 106). Их пересобрать как тесты Rust, а ядро сверять с Python.

## Стратегия

1. **Контракт API сохраняется.** Vue, `usePlayer`, `audio.rs` не трогаем; Python и Rust можно запускать рядом.
2. **Сначала крейт без Tauri** (`radio-core`), интеграция потом.
3. **Golden-тесты.** Одни и те же WAV прогоняются через Python и Rust: сравниваются BPM, тональность, LUFS, точки
   перехода и PCM перехода (допуск по SNR).
4. **Бэкенды по сложности:** `local` → Яндекс → YouTube.

## Декомпозиция

### Этап 0. Лицензия, каркас, эталон
- **0.1** ✅ Сменить лицензию на AGPL-3.0-or-later.
- **0.2** ✅ Крейт `crates/radio-core` (отдельный пакет без workspace: `src-tauri` подключит его по `path`, сборку Tauri не трогаем). Пока только `serde`; tokio/axum добавятся с этапами 3–4.
- **0.3** ✅ (только `analyze()`; PCM переходов появятся с 1.7) Генератор эталонов `crates/radio-core/tools/gen_golden.py`, 10 случаев, сигналы не в git: Python `analyze()` и `blend()` на синтетике из тестов радио и 5–10 реальных
  треках → JSON и PCM-фрагменты в `tests/golden/`.
- **0.4** ✅ (не запускался) `.github/workflows/rust.yml`: ставит Python-радио с Essentia, генерирует сигналы, `cargo test` с `GOLDEN_REQUIRED=1`.
- **0.5** Ссылка на исходники в Settings ✅. `THIRD-PARTY-LICENSES` (`cargo-about`, `license-checker`) в пакеты — не сделано.

### Этап 1. Аудио-ядро (`mix/`), самый рискованный
- **1.1** ✅ Типы `Pcm`, `TrackAnalysis`, константы, `SAMPLE_RATE`.
- **1.2** ✅ Декодирование файла в f32-стерео (подпроцесс ffmpeg; `symphonia` позже, если уберём ffmpeg).
- **1.3** ✅ (bpm, биты, диапазон и секции совпадают с Python на всех 10 эталонах; в Python падение на кусках 2048…23000 отсчётов, в Rust нет) Темп: `onset_envelope`, `estimate_tempo`, `refine_tempo`, `find_beat`, `find_sections`, `audible_range`.
- **1.4** ✅ Громкость: `ebur128`; RMS+2 dB для коротких кусков, -70 для тишины (как у Essentia). Расхождение с Essentia до 0,22 дБ на басовых сигналах (ниже ~100 Гц Essentia давит низ сильнее, 30 Гц: 1,3 дБ; подгонка фильтра не получилась). Точные числа даст `LoudnessEBUR128` самой Essentia через C-обёртку из 1.5.2; `ebur128` остаётся fallback без feature `essentia`.
- **1.5** ✅ Тональность и точная громкость (feature `essentia`):
  - **1.5.1** ✅ Essentia собирается без waf: `build.rs` компилирует крейтом `cc` ядро, 14 нужных алгоритмов
    (список в `native/gen_registry.py`), KissFFT и обёртку. Нужны `vendor/essentia` (коммит `dc91e47`) и
    `vendor/eigen` 3.3.7 (скачивает `tools/fetch-vendor.sh`). Сборка около 35 с на Apple clang; `g++` в CI не проверен.
  - **1.5.2** ✅ C-обёртка `native/shim.cpp` (`ew_key`, `ew_loudness`) и `mix/essentia.rs`.
  - **1.5.3** ✅ Тональность совпадает с Python на 34 эталонах, включая все 24 тональности; громкость до 0,001 дБ.
  - **1.5.4** ✅ Запасной путь без Essentia или при её отказе: профили Крумхансла (`mix/chroma.rs`), хрома и
    уверенность совпадают с Python.
  - **1.5.5** Раздел лицензий в README: Essentia теперь в самом приложении (AGPL, как и сам плеер) — не сделано.
- **1.6** ✅ Планирование (`mix/plan.rs`: `plan_matched`, `plan_early`, `plan_blend`, `tempo_ratio`, `transition_speed`, `section_length`, `choose_transition`; 300 пар, 1800 запросов `plan_early` — те же числа, что у Python): `plan_matched`, `plan_early`, `plan_blend`, `tempo_ratio`, `transition_speed`.
- **1.7** ✅ (`mix/blend.rs`, 8 блендов + `swap_point`; SNR к Python 137–151 дБ) DSP переходов: `crossfade`, `bass_swap`, `eq_blend`, `filter_sweep`, `_lowpass`, `swap_point`.
- **1.8** ✅ (`mix/glide.rs`; 4 glide-кейса, выход побитно совпадает с Python, сетка долей и атаки тоже) `_wsola` и `tempo_glide` (растяжение по времени, поиск атак). Нужен эталон PCM и прослушивание.
- **1.9** ✅ (`mix/effects.rs`: эффекты, лимитер, `blend()`; 19 случаев с воспроизведённым шумом Python, SNR 134–336 дБ; шум за trait `NoiseSource`, в радио `RandomNoise` на `rand`) Эффекты (`riser`, `impact`, `echo_out`, `add_drop`) и look-ahead `limit`; ГСЧ с фиксированным сидом.
- **1.10** ✅ (`mix/choose.rs`, `mix/key.rs`; 80 случайных пулов, все 784 пары тональностей, 150 пар треков — совпадает с Python) Подбор следующего: `energy`, `arc_target`, `mood_*`, `transition_cost`, `_best_path`, `choose_next`,
  функции Camelot. Чистая логика, переносится по тестам `test_dj.py`.

### Этап 2. Кэш (`cache.rs`)
- **2.1** Схема SQLite `~/.cache/yamusic-radio.db` совместима с Python-версией (анализ и история не теряются).
- **2.2** Версионирование анализа: пересчёт, если тональность или громкость измерены другим способом.
- **2.3** Конфиг `~/.config/yamusic-radio.json` (токены, режим 0600).

### Этап 3. Радио и поток
- **3.1** Состояние `Radio`: очередь `previous/now/next`, пауза, `pinned`, `move`, `order`, `remove`, `play`,
  `refresh`.
- **3.2** Цикл `mix()` как поток `Piece`; подготовка следующей деки за ~40 с до перехода; «ранний» план при
  смене очереди.
- **3.3** Кодирование: trait `Encoder` + реализация на подпроцессах ffmpeg (MP3, FLAC, Ogg), `Broadcaster` с preroll 3 с; парсеры границ кадров
  `mp3_start`, `flac_start`, `ogg_start`, CRC8, Ogg CRC — один в один.
- **3.4** Остановка кодировщиков, пауза «нет слушателей».
- **3.5** `spectrum.rs`: 12 полос, SSE `/api/spectrum`, событие `start`, привязка слушателя по `?l=`.

### Этап 4. HTTP API
- **4.1** Маршруты `axum` с теми же JSON-формами, что в README радио; контрактные тесты против живого Python.
- **4.2** `/api/queue`, `/api/next`, `/api/pause`, `/api/resume`, `/api/moods`, `/api/genres`, `/api/language`,
  `/api/stats`, `/api/stats/keys`, `/api/transitions`, `/api/auth*`.
- **4.3** CORS `*` для потока и спектра, ICY-заголовки, `GET /` → 404 с JSON.

### Этап 5. Бэкенды
- **5.1** trait `MusicBackend` и `MultiBackend`: общий пул треков, отключение, откат после сбоев (30 с → 10 мин).
- **5.2** `local`: обход папки, `track_id` (хэш), теги через ffmpeg, случайный порядок без повторов,
  пересканирование раз в 10 минут.
- **5.3** Яндекс: OAuth device flow, rotor-станция, настроение и язык, HMAC-подпись ссылки загрузки, `Cooldown`
  и retry при rate limit. Запросы сверять с трафиком Python-версии (API неофициальный).
- **5.4** (отложен) YouTube: InnerTube-запросы + `yt-dlp` внешним бинарём рядом с ffmpeg (нужен JS-рантайм). Последним.

### Этап 6. Интеграция в плеер
- **6.1** Запуск `radio-core` внутри `src-tauri` (задача/поток) вместо дочернего процесса; заменяет `radio.rs`.
- **6.2** Сохранить `ENDLESSWAVE_PORT`, `ENDLESSWAVE_ARGS`, `ENDLESSWAVE_LOG/DEBUG` и «если на порту уже есть
  сервис, подключиться».
- **6.3** Убрать PyInstaller: `scripts/build-backend.sh`, `backend/`, шаги в `release.yml`; обновить раздел
  Third-party в README.
- **6.4** Reset в настройках работает через общий `cache.rs`.

### Этап 7. Проверка и переключение
- **7.1** A/B на реальной музыке: час эфира Python против Rust (пики лимитера, щелчки на стыках, дрейф темпа).
- **7.2** Нагрузка и память (Python держит около 130 МБ при анализе шестиминутного трека).
- **7.3** Релиз под флагом `ENDLESSWAVE_ENGINE=rust`, затем по умолчанию, Python-бинарь убрать через 1–2 релиза.

### Этап 8. Убрать ffmpeg (после этапа 7)
- **8.1** Чтение файлов: `symphonia` вместо `decode_file` через ffmpeg; теги `local`-бэкенда оттуда же.
- **8.2** Встроенные энкодеры за trait `Encoder`: `mp3lame-encoder` (MP3), `flacenc` (FLAC).
- **8.3** Ogg-FLAC для Safari: свой Ogg-контейнер (страницы, granule, CRC) поверх `flacenc`.
- **8.4** Сверка с потоком ffmpeg (границы кадров, preroll, ICY), убрать `build-ffmpeg.sh`, `ffmpeg/` и LGPL-заметку
  из пакетов.

## Зависимости между этапами

```
0 → 1.1–1.3 → 1.4 / 1.5 ┐
      1.6 → 1.7 → 1.8 → 1.9 ┤→ 3 → 4 → 6 → 7
0 → 1.10, 2 ────────────────┘
5.1–5.2 → 3 (нужен хоть один бэкенд);  5.3 → после 4;  5.4 → последним
```

**Минимальный рабочий срез:** этапы 0–4 + бэкенд `local` (5.1–5.2) — радио из папки на Rust без Яндекса и YouTube.

## Риски

1. **Тональность (1.5):** сборка C++-Essentia в CI на двух платформах; без неё подбор гармоничных переходов хуже.
2. **WSOLA и эффекты (1.8–1.9):** численная эквивалентность проверяется только частично, нужно слушать.
3. **Яндекс API (5.3):** неофициальный, мелочь в подписи ломает загрузку.
4. **YouTube (5.4):** нет аналогов `ytmusicapi`/`yt-dlp` на Rust.
5. **Объём:** около 8 000 строк с численным кодом, Python и Rust живут рядом всю миграцию.
