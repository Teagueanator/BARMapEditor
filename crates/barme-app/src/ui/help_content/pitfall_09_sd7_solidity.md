# PITFALL §9 — `.sd7` must be non-solid

7-Zip's "solid" mode bundles multiple files into a single LZMA
stream, which gives better compression but makes random-access
extraction slow. SpringFiles, the BAR community indexer,
silently rejects solid archives.

## Rule

The packager invokes `7z` with `-ms=off`. An integration test
opens the output and asserts `IsSolid == false`.

## Symptoms

If you upload a solid `.sd7` to SpringFiles:

- The archive is silently dropped from the index.
- No error email; no rejection notice.
- Other users can't discover it through the lobby browser.

Local maps (dropped into BAR's user maps directory) work fine
regardless of solidity — this is purely a SpringFiles
distribution gate. The editor's Build + Install path always
emits non-solid archives.

## How to verify by hand

```sh
7z l -slt your_map.sd7 | grep '^Solid '
```

Should print `Solid = -` (i.e. not solid). `Solid = +` means
SpringFiles will reject it.
