---
title: Project keys
description: Keys for screens supplied by project plugins.
---

Press `p` from the tree or a preview to open a detected project. Several matches open a chooser;
use `j`/`k` and `Enter`, or `Esc` to cancel. The existing git/jj screens remain the fallback.

| Key | Action |
| --- | --- |
| `Tab` / `Shift-Tab` | Cycle screens |
| A screen's declared key | Jump to that screen |
| `j` / `k`, arrows | Move or scroll |
| `V` | Toggle a row mark |
| `/` | Filter the list |
| `Enter` | Run the row kind's default action |
| `Space` / `?` | Matching actions / working keys |
| `q` / `Esc` | Pop a screen or return to the original view |

Confirmation keys and choices come from the plugin. Text prompts include the initial value.
`Esc` in any prompt cancels without running the action.

Project operations typed while a view is loading wait for its reply. Screen navigation cancels
pending row input. A screen-jump key wins a collision inside project screens; the action keeps
its key elsewhere and remains available through the `Space` menu and help.
