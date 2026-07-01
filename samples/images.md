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

## Remote images (fetched with the system `curl`, cached on disk)

A remote raster screenshot and an SVG badge — the kind READMEs show on GitHub.
Both are downloaded in the background and rendered inline (SVG is rasterized):

![remote raster](https://placehold.co/480x160.png)

![build badge](https://img.shields.io/badge/konoma-preview-brightgreen.svg)

## Safe fallbacks (design principle #3)

An unreachable remote URL and a missing local file degrade to a text
placeholder instead of breaking the preview:

![unreachable host](https://konoma.invalid/nope.png)

![missing file](does-not-exist.png)

End of demo.
