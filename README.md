# diagram-render

Turn Mermaid, Graphviz DOT, D2, BPMN, draw.io and Excalidraw files into SVG or PNG on your own computer, with no browser and no server, and check them for errors by line.

## Install

Windows (PowerShell, no admin; installs to `%LOCALAPPDATA%\Programs\diagram-render` and adds it to your user PATH):

```powershell
irm https://github.com/nowmyagentcan/diagram-render/releases/latest/download/install.ps1 | iex
```

macOS and Linux (installs to `~/.local/share/diagram-render`, linked from `~/.local/bin`):

```sh
curl -fsSL https://github.com/nowmyagentcan/diagram-render/releases/latest/download/install.sh | sh
```

| System | Download |
|---|---|
| Windows x64 | [diagram-render-v0.1.0-windows-x64.zip](https://github.com/nowmyagentcan/diagram-render/releases/download/v0.1.0/diagram-render-v0.1.0-windows-x64.zip) |
| macOS Apple silicon | [diagram-render-v0.1.0-macos-arm64.tar.gz](https://github.com/nowmyagentcan/diagram-render/releases/download/v0.1.0/diagram-render-v0.1.0-macos-arm64.tar.gz) |
| macOS Intel | [diagram-render-v0.1.0-macos-x64.tar.gz](https://github.com/nowmyagentcan/diagram-render/releases/download/v0.1.0/diagram-render-v0.1.0-macos-x64.tar.gz) |
| Linux x64 | [diagram-render-v0.1.0-linux-x64.tar.gz](https://github.com/nowmyagentcan/diagram-render/releases/download/v0.1.0/diagram-render-v0.1.0-linux-x64.tar.gz) |
| Linux arm64 | [diagram-render-v0.1.0-linux-arm64.tar.gz](https://github.com/nowmyagentcan/diagram-render/releases/download/v0.1.0/diagram-render-v0.1.0-linux-arm64.tar.gz) |

Each archive holds everything it needs: the program, three engines, the fonts and the licences. Keep the folder together. To remove it, delete the folder.

## Use

```sh
$ diagram-render flow.mmd -o flow.png
✓ flow.mmd → flow.png  1378×374  13 KB  0.17 s

$ diagram-render check flow.mmd
✓ flow.mmd is valid Mermaid (flowchart)
```

The format comes from the file name or its content. Without `-o`, `flow.svg` is written beside the input.

```sh
diagram-render graph.dot --to png --scale 3
diagram-render board.drawio --page 2 -o page2.svg
diagram-render sketch.excalidraw --to png --background transparent
echo "graph TD; A-->B" | diagram-render - --format mermaid -o ab.png
diagram-render formats
```

`diagram-render` alone starts an interactive prompt: type or drop a file, add `png` for a PNG.

`diagram-render ui` opens a local page to paste or drop a diagram and see it, and double-clicking the Windows exe does the same. The page runs on 127.0.0.1 and writes nothing to disk.

### Use with AI agents

`--json` on `render`, `check` and `formats` prints one JSON object; errors are `{"error": {"code", "message", "hint"}}`, with `line`, `column` and `excerpt` when the problem has a place in the source.

```sh
diagram-render check flow.mmd --json            # errors with line and column
diagram-render flow.mmd -o flow.png --json      # writes flow.png; then open it to look
cat graph.dot | diagram-render - --format dot -o graph.svg --json
```

`diagram-render mcp` is a local MCP server on stdio with two tools: `check_diagram`, and `render_diagram`, which returns a PNG preview so the model can see what it drew.

| Agent | Setup |
|---|---|
| Claude Code | `claude mcp add diagram-render -- diagram-render mcp` |
| Codex | `codex mcp add diagram-render -- diagram-render mcp` |
| Cursor (`~/.cursor/mcp.json`), Claude Desktop | `{"mcpServers":{"diagram-render":{"command":"diagram-render","args":["mcp"]}}}` |
| VS Code (`.vscode/mcp.json`) | `{"servers":{"diagram-render":{"type":"stdio","command":"diagram-render","args":["mcp"]}}}` |

## Supported inputs and limits

| Format | Files | Drawn by |
|---|---|---|
| Mermaid | `.mmd` `.mermaid` | merman 0.9.3; `diagram-render formats --json` lists its diagram families |
| Graphviz DOT | `.dot` `.gv` | Graphviz 16.1.0, all 8 layouts |
| D2 | `.d2` | d2 0.9.0, with dagre, elk and tala |
| BPMN 2.0 | `.bpmn` `.bpmn2` | this tool, from the file's diagram section |
| draw.io | `.drawio` `.dio` `.drawio.xml` `.drawio.svg` `.drawio.png` | this tool |
| Excalidraw | `.excalidraw` `.excalidraw.json` `.excalidraw.svg` `.excalidraw.png` | this tool |

- Input: one diagram per run, up to 64 MiB, and 4 MiB for Mermaid, DOT and D2 text.
- Output: SVG or PNG. PNGs stop at 16384 px a side and 64 MP; a larger diagram comes out at a lower scale, with a warning.
- Every SVG loses its scripts, event handlers and `javascript:` links.
- draw.io vendor shape libraries (such as `mxgraph.aws4.*`) and stencils are drawn as dashed boxes with their labels, and the run warns about them.
- A BPMN file with no diagram section isn't laid out; open it in a BPMN modeler and save it to add one.
- Text in PNGs uses the bundled fonts. Characters they don't have, such as Chinese, Japanese or Korean, come from this computer's fonts, with a warning, so those images can differ between computers.
- Graphviz `image=` attributes depend on the system: Graphviz needs an image library to read their size, and only the Windows build has one.
- Not read: PlantUML, Visio, or Markdown files with diagrams inside. Not written: PDF, JPEG or animated SVG.
- It makes no network requests. The one exception is D2 with `--allow-network`, which fetches the image URLs written in your diagram. Without it, D2 SVGs keep those images as links, and D2 PNGs that need them stop with an error saying so.

## How it works

Mermaid, DOT and D2 go to [merman](https://github.com/Latias94/merman), [Graphviz](https://graphviz.org) and [D2](https://github.com/d2lang/d2), each an unmodified program in `engines/`, run with a clean environment and a time limit.
BPMN, draw.io and Excalidraw are drawn by this tool's own Rust renderers, with geometry ported from [mxGraph](https://github.com/jgraph/drawio), [Excalidraw](https://github.com/excalidraw/excalidraw) and [rough.js](https://github.com/rough-stuff/rough).
PNGs come from [resvg](https://github.com/linebender/resvg), or from merman's and d2's own rasterisers for their diagrams.

## First run on an unsigned build

Windows: if "Windows protected your PC" appears, choose More info, then Run anyway.
The install script doesn't trigger it; a zip extracted in Explorer may.

macOS: if it says the developer can't be verified, open System Settings, Privacy & Security, and choose Open Anyway.
Every engine in a browser-downloaded archive carries the same flag, so clear them at once with `xattr -dr com.apple.quarantine <unpacked folder>`.
The install script doesn't trigger it, because curl downloads carry no quarantine flag.

## Licence

Apache-2.0, see [LICENSE](LICENSE). [NOTICE](NOTICE) lists the bundled engines with their sources and the code ported into this repository. Each release archive includes every third-party licence in `licenses/`.

---

[nowmyagentcan.com](https://nowmyagentcan.com) · [x.com/chayp05](https://x.com/chayp05)
