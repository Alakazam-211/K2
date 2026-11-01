Test speech for kessel-audio's metric tests: 12 LibriSpeech utterances, Opus-encoded.

# Source and licence

`librispeech-test-clean.kaop` holds speech from the LibriSpeech ASR corpus
(V. Panayotov, G. Chen, D. Povey and S. Khudanpur, "LibriSpeech: an ASR
corpus based on public domain audio books", ICASSP 2015), test-clean
subset, from <https://www.openslr.org/12>. LibriSpeech is licensed
**CC BY 4.0** (<https://creativecommons.org/licenses/by/4.0/>). The
recordings come from LibriVox audiobooks.

Changes made: each utterance was resampled from 16 kHz to 48 kHz,
truncated to at most 12 s, scaled to an active speech level of -26 dBFS,
and encoded as Opus (mono, 32 kbit/s, 20 ms frames).

| Pool | Speaker | Length | Utterance |
|---|---|---|---|
| far | 1089 | 10.44 s | 1089-134686-0000 |
| far | 1188 | 10.72 s | 1188-133604-0000 |
| far | 121 | 8.46 s | 121-121726-0000 |
| far | 1221 | 12.00 s | 1221-135766-0000 |
| far | 1089 | 3.27 s | 1089-134686-0001 |
| far | 1188 | 9.04 s | 1188-133604-0001 |
| near | 2094 | 12.00 s | 2094-142345-0000 |
| near | 2300 | 5.08 s | 2300-131720-0000 |
| near | 237 | 12.00 s | 237-126133-0000 |
| near | 260 | 7.04 s | 260-123286-0000 |
| near | 2094 | 8.03 s | 2094-142345-0001 |
| near | 2300 | 12.00 s | 2300-131720-0001 |

# Format (`.kaop`)

Little-endian. `KAOP`, a version byte (1), a `u16` utterance count; then per
utterance: `u8` pool (0 far-end, 1 near-end), `u32` speaker, `u32` length in
48 kHz samples, `u32` packet count, and per packet a `u16` length and the
Opus packet. The tests decode it with the crate's own Opus decoder.
