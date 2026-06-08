# Speech-to-Text for a Push-to-Talk Dictation App (Rust, Win+Mac) — 2025/2026

**Use case:** short utterances (seconds to ~1–2 min), push-to-talk (record-while-held, transcribe-on-release), prioritizing **low latency + accuracy**. This profile differs from long-form transcription.

> **Confidence:** vendor STT pricing shifted through 2025–2026 and third-party blogs disagree. Official pages cited as canonical; spreads flagged. Latency figures are vendor/benchmark claims — benchmark on target laptops.

---

## 1. Deepgram (Cloud Streaming)

**Models:** Nova-3 flagship (mono + multilingual); also new **Flux** streaming-optimized line.

**Pricing (official, canonical):**
| Mode | Nova-3 Mono | Nova-3 Multi |
|---|---|---|
| Streaming (PAYG) | **$0.0048/min** | $0.0058/min |
| Streaming (Growth) | $0.0042/min | $0.0050/min |
| Pre-recorded/Batch (PAYG) | **~$0.0077/min** | ~$0.0092/min |

⚠️ **Price conflict:** third-party sources cite $0.0043–$0.0077/min. The widely-quoted "$0.0077/min = $0.46/hr" appears to be the **batch** rate; **streaming cheaper (~$0.0048/min)**. Verify on live dashboard.

**Latency:** time-to-first-token **<300 ms** (~200–300 ms reported); interim transcripts <1 s. Genuine differentiator.
**Accuracy:** Deepgram WER ~5.3% clean; third-party ~18% noisy/accented. Nova-3 adds **keyterm prompting** (domain vocab boost).
**Languages:** ~36–45+ (multilingual); mono is English-optimized.
**Rust:** no first-party SDK — raw WebSocket (tokio-tungstenite) for streaming, HTTPS POST for batch. Straightforward JSON-over-WS + PCM/Opus.

**Streaming vs send-on-release (the key PTT question):**
- Streaming wins perceived latency only if you display interim results live. On release, finalized transcript ~200–300 ms later.
- Send-on-release (batch) simpler, slightly cheaper per current table; a 10 s clip finalizes well under a second after release.
- **Verdict: for short PTT without mid-speech display, send-whole-clip-on-release is the better tradeoff.** Stream only for live word-by-word feedback.

---

## 2. OpenAI + AssemblyAI (brief)

**OpenAI:**
| Model | Price | Notes |
|---|---|---|
| `whisper-1` | $0.006/min | Batch only |
| `gpt-4o-transcribe` | $0.006/min | ~4.1% WER; "near real-time" |
| `gpt-4o-mini-transcribe` | **$0.003/min** | Cheapest; good accuracy |
| `gpt-realtime` STT (streaming) | ~$0.017/min | True realtime WS |

- Latency: Whisper/gpt-4o-transcribe via HTTP chunked ~500–1500 ms first chunk — slower first-token than Deepgram. Sub-200 ms needs pricier realtime.
- **`gpt-4o-mini-transcribe` = cheapest accurate cloud batch ($0.003/min)**, strong for send-on-release. Deepgram leads for low-latency live.

**AssemblyAI:** competitive accuracy, real-time Universal-Streaming (~$0.15–0.47/hr), diarization. Reasonable Deepgram alt; raw WS; not clearly better for this case.

---

## 3. Local — whisper.cpp / whisper-rs (and alternatives)

**`whisper-rs`** = maintained Rust binding over **whisper.cpp** (ggml). Most practical local path: single C/C++ dep, CPU/**Metal (Mac)**/CUDA/Vulkan — only realistic option covering Win+Mac with GPU accel from one codebase.

**Model sizes / RAM:**
| Model | Params | RAM | Use |
|---|---|---|---|
| tiny | 39M | ~1 GB | fast, low acc |
| base | 74M | ~1 GB | light |
| small | 244M | ~2 GB | good balance |
| medium | 769M | ~5 GB | high acc |
| large-v3 | 1.55B | ~3.9–10 GB | best |
| **large-v3-turbo** | distilled | ~4–6 GB | **~8× faster than large-v3, near-large acc** ⭐ |
| distil-large-v3 | ~half | ~4 GB | ~6× faster, −1% WER (En) |
(int8 GPU quant → large-v3 ~2.5 GB VRAM.)

**Speed (× real-time, large-v3):** Mac Metal (M-series) ~10×; CUDA (RTX 4070) ~8× (faster-whisper ~12×); CPU (base) ~15–20×.

**Realistic latency, 10 s clip (estimate):** Apple Silicon + Metal `large-v3-turbo` **~0.5–1.5 s**; mid Windows CPU `small` ~1–3 s; `large-v3` CPU-only 5–15 s+ (often too slow). **`large-v3-turbo`/`distil-large-v3` = sweet spot.**

**Alternatives:** **Parakeet (TDT v2/v3)** extremely fast, excellent English, but NVIDIA-GPU oriented (CUDA-only build). **Moonshine (245M)** purpose-built for **short utterances + streaming**, matches Whisper-large En at ~6× fewer params — strong PTT candidate if English-only OK (~8 langs); Rust bindings less mature, verify. **faster-whisper** faster on NVIDIA (CTranslate2) but Python — awkward to embed; prefer whisper.cpp.

---

## 4. Recommendation Matrix
| Dimension | Deepgram (stream) | OpenAI gpt-4o-mini (batch) | whisper.cpp local (turbo) |
|---|---|---|---|
| First-token latency | ★★★★★ (<300 ms) | ★★★ (0.5–1.5 s) | ★★★★ (no RTT; ~0.5–2 s compute) |
| Accuracy (clean) | ★★★★ | ★★★★★ | ★★★★ |
| Cost | ★★★★ (~$0.005/min) | ★★★★★ ($0.003/min) | ★★★★★ (free) |
| Privacy | ★★ | ★★ | ★★★★★ |
| Offline | ✗ | ✗ | ★★★★★ |
| Rust | raw WS | raw HTTPS | `whisper-rs` native |

**When:** Cloud (Deepgram) for lowest live latency + consistent quality on any HW + minimal footprint (always-online). Local (whisper.cpp + turbo) for privacy/offline, $0/min, decent HW — natural default for a desktop dictation tool.

**Recommended hybrid (shipping product):**
1. Default **local whisper.cpp via whisper-rs + `large-v3-turbo`** (Metal/CUDA/CPU). Private, no cost, no network.
2. Offer **cloud opt-in** (Deepgram or gpt-4o-mini) for weak HW / max accuracy. gpt-4o-mini cheapest accurate batch; Deepgram for live interim words.
3. Auto-select model size by detected hardware.
**→ This is Holler's STT decision.**

---

## 5. Voice Activity Detection (trim silence)
| VAD | Accuracy (TPR @5% FPR) | CPU | Notes |
|---|---|---|---|
| WebRTC VAD | ~50% | very light | fast, cuts speech in noise |
| Silero VAD | ~87.7% | moderate (ONNX) | accurate; few-hundred-ms detection delay |
| Cobra (Picovoice) | ~98.9% | very light | best, but commercial |

**For PTT:** key press/release defines utterance boundaries, so VAD's job is **silence trimming**, not endpointing. Use `wavekat-vad` (unified trait WebRTC/Silero) or Silero directly. Don't over-engineer — trim + small padding. WebRTC if minimizing deps. Silero's detection lag irrelevant (full clip already in hand).

---

## Bottom line
**Ship local-first:** `whisper-rs` (whisper.cpp) `large-v3-turbo`, Metal/CUDA/CPU auto-selected, Silero VAD silence trim, **transcribe-on-release** (not streaming — utterances short). Private, offline, free, ~0.5–2 s on modern HW. **Add cloud opt-in** (Deepgram Nova-3 streaming for sub-300 ms live words, or gpt-4o-mini-transcribe $0.003/min cheapest batch) via raw WS/HTTPS.

**Verify yourself:** (1) exact Deepgram streaming vs batch price (sources conflict $0.0043–$0.0077/min); (2) on-device latency on actual target laptops; (3) Moonshine Rust binding maturity if wanted.

### Sources
Deepgram: deepgram.com/pricing, /learn pricing-breakdown-2025, brasstranscripts.com, transcriber.talkflowai.com, deepgram.com/learn STT benchmarks. OpenAI: openai.com/api/pricing, costgoat.com, tokenmix.ai. whisper.cpp: github.com/ggml-org/whisper.cpp, promptquorum.com local STT comparison 2026, openwhispr.com model sizes, modal.com whisper variants. Parakeet/local: developer.nvidia.com blog, onresonant.com, northflank.com best OSS STT 2026. VAD: github.com/wavekat/wavekat-vad, picovoice.ai best VAD 2026.
