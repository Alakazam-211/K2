# k2-multiplayer-markdown

The Markdown type pack for the K2 multiplayer kit
([k2-multiplayer-base](https://github.com/Alakazam-211/k2-multiplayer-base)). People and agents
edit the same `.md` file live, with presence and attribution, and the file on disk stays plain
Markdown, byte for byte.

- **Schema:** one text root (`source`) over the Markdown source, so every dialect survives.
- **Codec:** an exact BOM/CRLF round trip, a 4 MiB limit, and decline rules; fidelity tests
  run on a corpus.
- **Agent parity:** `docs/PARITY.md` maps every person action to a command-line verb.

Check this repo out next to `k2-multiplayer-base` (sibling directories).
Status: early. Licence: not yet chosen.
