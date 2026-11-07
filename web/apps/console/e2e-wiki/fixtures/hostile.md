# Hostile page

<script>window.__pwned = "script tag";</script>

<script src="https://evil.example/remote.js"></script>

Inline <script>window.__pwned = "inline script";</script> script.

<img src="https://evil.example/raw-img.png" onerror="window.__pwned = 'img onerror'">

<img src=x onerror="window.__pwned = 'img onerror local'">

<svg onload="window.__pwned = 'svg onload'"><script>window.__pwned = 'svg script'</script></svg>

<iframe src="https://evil.example/frame.html"></iframe>

<iframe srcdoc="<script>parent.__pwned = 'srcdoc'</script>"></iframe>

<object data="https://evil.example/object.swf"></object>

<embed src="https://evil.example/embed.swf">

<video src="https://evil.example/video.mp4" autoplay onplay="window.__pwned = 'video'"></video>

<audio src="https://evil.example/audio.mp3" autoplay></audio>

<link rel="stylesheet" href="https://evil.example/style.css">

<style>@import url("https://evil.example/import.css"); body { background: url("https://evil.example/bg.png"); }</style>

<meta http-equiv="refresh" content="0; url=https://evil.example/refresh">

<base href="https://evil.example/">

<form action="https://evil.example/form"><input name="x" autofocus onfocus="window.__pwned = 'autofocus'"><button>go</button></form>

<div onmouseover="window.__pwned = 'mouseover'" onclick="window.__pwned = 'click'" style="background:url(https://evil.example/style-attr.png)">event handlers</div>

<a href="javascript:window.__pwned='raw anchor'">raw anchor</a>

[plain javascript](javascript:window.__pwned='link')

[mixed case](JaVaScRiPt:window.__pwned='mixed')

[entity encoded](&#106;avascript:window.__pwned='entity')

[percent space](java%0Ascript:window.__pwned='percent')

[leading space]( javascript:window.__pwned='space')

[vbscript](vbscript:window.__pwned='vb')

[data html](data:text/html;base64,PHNjcmlwdD5wYXJlbnQuX19wd25lZD0nZGF0YSc8L3NjcmlwdD4=)

[file](file:///etc/passwd)

<javascript:window.__pwned='autolink'>

[reference javascript][bad]

[bad]: javascript:window.__pwned='reference'

![remote image](https://evil.example/md-image.png)

![image with title](https://evil.example/md-image-title.png "a title")

![reference image][pic]

[pic]: https://evil.example/ref-image.png

[![image in a link](https://evil.example/linked-image.png)](https://evil.example/linked)

![data image](data:image/svg+xml,%3Csvg%20onload%3D%22window.__pwned%3D1%22%2F%3E)

[[Some page|<img src="https://evil.example/label.png" onerror="window.__pwned='wiki label'">]]

[[<script>window.__pwned='wiki target'</script>]]

[[javascript:window.__pwned='wiki js']]

{{claim:<script>window.__pwned='claim'</script>}}

[a real external link](https://example.org/safe "title <b>bold</b>")

`<script>window.__pwned = "inline code"</script>`

```html
<script>window.__pwned = "fenced"</script>
```

| cell | `<img src=x onerror=1>` |
|---|---|
| <img src="https://evil.example/table.png"> | ![t](https://evil.example/table-md.png) |

- [x] <b onclick="window.__pwned='task'">task</b>

&lt;script&gt;window.__pwned = "entity script"&lt;/script&gt;

&#60;img src=x onerror=&#34;window.__pwned='entity img'&#34;&#62;
