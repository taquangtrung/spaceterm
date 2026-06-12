# spaceterm-client (Python)

Emit rich [Terminal Block Protocol](../../docs/terminal-block-protocol-spec.md)
(TBP) blocks from Python. Falls back to `text/plain` when SpaceTerm is not the active
terminal, so scripts stay safe under tmux, ssh, and CI.

```python
import spaceterm

spaceterm.display(dataframe)                  # uses the object's _repr_*_ methods
spaceterm.display_svg(open("plot.svg").read())
spaceterm.display_image("chart.png")
spaceterm.display_markdown("# hello")
```

## Develop

```bash
cd clients/client-py
python -m pytest          # tests (pythonpath=src is configured)
ruff check .
```
