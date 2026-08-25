# Inline images (Markdown & HTML)

konoma renders block-level images inline, in the flow of the document (kitty
graphics). Local images and remote (`http(s)://`) images both work — remote
ones download off-thread and show a "loading" line until they arrive. Images
scroll with the text.

## Markdown `![alt](path)`

![konoma sample image](sample.png)

Text between the two images, so you can see the spacing and scrolling behavior.

![a smaller sample](sample.jpg)

## HTML `<img>` (the same form the README uses)

<p align="center"><img src="sample.png" width="480" alt="html image form"></p>

## Animated GIF

Inline GIFs cycle in place, the same way the full-screen preview animates them:

![animated sample](sample.gif)

## Remote images (fetched with the system `curl`, cached on disk)

A remote raster screenshot and an SVG badge — the kind READMEs show on GitHub.
Both are downloaded in the background and rendered inline (SVG is rasterized):

![remote raster](https://placehold.co/480x160.png)

![build badge](https://img.shields.io/badge/konoma-preview-brightgreen.svg)

## Images in a table cell

A table cell holds a real image too, in both kinds of table. The cell reserves a rectangle sized
to its own column, so the picture lands inside the box drawing rather than beside it:

| Format | Sample |
|--------|--------|
| JPEG   | ![a raster sample](sample.jpg) |
| SVG    | ![a vector sample](sample.svg) |

The same thing written as an HTML `<table>` — the screenshot-grid form, pictures on top and a
caption row underneath, which is how this repository's own README lays out its screenshots:

<table>
  <tr>
    <td><img src="sample.jpg" alt="a raster sample"></td>
    <td><img src="sample.svg" alt="a vector sample"></td>
  </tr>
  <tr>
    <td align="center"><b>JPEG</b> — decoded to pixels</td>
    <td align="center"><b>SVG</b> — rasterized by resvg</td>
  </tr>
</table>

A remote image works in a cell as well, downloaded off-thread like any other:

| Source | Badge |
|--------|-------|
| shields.io | ![build badge](https://img.shields.io/badge/konoma-preview-brightgreen.svg) |

## Safe fallbacks (design principle #3)

An unreachable remote URL and a missing local file degrade to a text
placeholder instead of breaking the preview:

![unreachable host](https://konoma.invalid/nope.png)

![missing file](does-not-exist.png)

End of demo.
