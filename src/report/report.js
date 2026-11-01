// Draws the report's payload into the panels the shell left empty.
//
// Every decision is already in the payload: a cell's tone, an account's
// class, a bar's widths, a chart's coordinates. What is here is layout only.
// Owner text goes in through `textContent` and nothing else; the one
// `innerHTML` is a chart's SVG, which Rust built and escaped; its readout's
// figures and coordinates come from the payload like everything else.
(function () {
  "use strict";
  var D = JSON.parse(document.getElementById("d").textContent);

  function el(tag, cls, text) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }

  // A cell is bare text, a `[text, tone]` pair, or an index into the account
  // table. A tone is a class, or a `#rrggbb` -- the funding ramp's, which is
  // continuous and so has no class to name.
  function fill(e, c) {
    if (typeof c === "number") {
      var a = D.accounts[c];
      e.textContent = a[0];
      e.classList.add(a[1]);
    } else if (Array.isArray(c)) {
      e.textContent = c[0];
      if (c[1].charAt(0) === "#") e.style.color = c[1];
      else e.classList.add(c[1]);
    } else {
      e.textContent = c;
    }
    return e;
  }

  // A row is one grid; its class names the table, and the stylesheet places
  // each cell by position. A row with fewer cells than its table has columns
  // spans its last cell across the rest.
  function row(r, cls, cols) {
    var div = el("div", "row " + cls + (r.k ? " " + r.k : ""));
    if (r.d != null) {
      div.style.setProperty("--d", r.d);
      div.classList.add("sub");
    }
    var tint = null;
    r.c.forEach(function (c, i) {
      var cell;
      if (i === 0 && r.h != null) {
        cell = fill(el("button", "goal"), c);
        cell.type = "button";
        cell.dataset.h = r.h;
      } else {
        cell = fill(el("div"), c);
      }
      if (Array.isArray(c) && c[1].charAt(0) === "#") tint = c[1];
      if (i === r.c.length - 1 && r.c.length < cols) {
        cell.classList.add("span");
        cell.style.setProperty("--from", i + 1);
      }
      div.appendChild(cell);
    });
    if (r.m != null) {
      var meter = el("div", "meter");
      var bar = el("i");
      bar.style.width = r.m + "%";
      if (tint) meter.style.color = tint;
      meter.appendChild(bar);
      div.appendChild(meter);
    }
    return div;
  }

  function columns(head, rows) {
    return rows.reduce(function (n, r) {
      return Math.max(n, r.c.length);
    }, head.length);
  }

  function header(head, cls) {
    var div = el("div", "row hd " + cls);
    head.forEach(function (h) {
      div.appendChild(el("div", null, h));
    });
    return div;
  }

  function table(cls, head, rows) {
    var t = el("div", "tbl");
    var cols = columns(head, rows);
    if (head.some(function (h) { return h !== ""; })) t.appendChild(header(head, cls));
    rows.forEach(function (r) {
      t.appendChild(row(r, cls, cols));
    });
    return t;
  }

  // A ledger row carries only its day; the month's key completes the date.
  function ledgerRow(m, r, cols) {
    var dated = { c: [m.key + "-" + r.c[0]].concat(r.c.slice(1)), k: r.k };
    return row(dated, "lg", cols);
  }

  var picks = 0;
  function ledger(b) {
    var wrap = el("div");
    if (b.empty) {
      wrap.appendChild(el("p", "note", b.empty));
      return wrap;
    }
    var id = "month" + picks++;
    var label = el("label", "pick", "Month");
    label.htmlFor = id;
    var select = el("select");
    select.id = id;
    select.appendChild(new Option("All months", ""));
    b.months.slice().reverse().forEach(function (m) {
      select.appendChild(new Option(m.label, m.key, false, m.key === b.opens));
    });
    if (b.opens == null) select.value = "";
    var list = el("div", "tbl");
    function draw() {
      var pick = select.value;
      list.textContent = "";
      list.appendChild(header(b.head, "lg"));
      b.months.forEach(function (m) {
        if (pick && m.key !== pick) return;
        m.rows.forEach(function (r, i) {
          var div = ledgerRow(m, r, b.head.length);
          if (!pick && i === 0 && list.childNodes.length > 1) div.classList.add("sep");
          list.appendChild(div);
        });
      });
    }
    select.addEventListener("change", draw);
    draw();
    label.appendChild(select);
    wrap.appendChild(label);
    wrap.appendChild(list);
    return wrap;
  }

  function bar(b) {
    var wrap = el("div");
    var track = el("div", "bar");
    b.segments.forEach(function (s) {
      var seg = el("i");
      seg.style.width = s[0] + "%";
      seg.style.background = "var(--c" + s[1] + ")";
      track.appendChild(seg);
    });
    wrap.appendChild(track);
    wrap.appendChild(legend(b.legend.map(function (l) {
      var key = el("span", "key");
      key.style.background = "var(--c" + l[1] + ")";
      return [null, [key, el("span", null, l[0])]];
    })));
    return wrap;
  }

  // Each entry is `[tone class or null, parts]`; the tone colors the whole
  // entry, so a swatch in `currentColor` matches its name.
  // One `[tone, parts]` entry as an item: the legend's, and the readout's.
  function entry(tag, e) {
    var item = el(tag, "item");
    if (e[0]) item.classList.add(e[0]);
    e[1].forEach(function (part) {
      item.appendChild(part);
    });
    return item;
  }

  function legend(items) {
    var p = el("p", "legend");
    items.forEach(function (e) {
      p.appendChild(entry("span", e));
    });
    return p;
  }

  // A swatch in the entry's own `currentColor` beside its name: an account
  // or the Total, in the legend and in the readout alike.
  function keyed(c) {
    var name = fill(el("span"), c);
    var tone = name.className;
    name.className = "";
    return [tone, [el("span", "key"), name]];
  }

  // The namespace is read off the chart rather than written out, so the
  // script carries no URL for the page's no-external-request check to find.
  function mark(svg, tag, cls, attrs) {
    var e = document.createElementNS(svg.namespaceURI, tag);
    e.setAttribute("class", cls);
    Object.keys(attrs).forEach(function (k) {
      e.setAttribute(k, attrs[k]);
    });
    return e;
  }

  // The readout: the pointer snaps to the nearest month in `b.hover`, which
  // carries every coordinate and figure; a guide and a dot per line go into
  // the SVG in its own units, the figures into a box beside the guide. A
  // mouse leaving hides it; a finger lifting leaves it up until the next
  // touch lands outside the chart.
  function readout(plot, b) {
    var svg = plot.querySelector("svg");
    if (!svg || !b.hover.length) return;
    var box = svg.viewBox.baseVal;
    var layer = mark(svg, "g", "hover", {});
    var guide = mark(svg, "line", "guide", { y1: box.y, y2: box.y + box.height });
    layer.appendChild(guide);
    svg.appendChild(layer);
    var tip = el("div", "tip");
    tip.setAttribute("aria-hidden", "true");
    plot.appendChild(tip);
    var shown = -1;

    function hide() {
      shown = -1;
      plot.classList.remove("on");
    }

    function show(e) {
      var rect = svg.getBoundingClientRect();
      if (!rect.width) return;
      var at = box.x + (e.clientX - rect.left) / rect.width * box.width;
      var near = 0;
      b.hover.forEach(function (h, i) {
        if (Math.abs(h.x - at) < Math.abs(b.hover[near].x - at)) near = i;
      });
      plot.classList.add("on");
      if (near === shown) return;
      shown = near;
      var h = b.hover[near];
      guide.setAttribute("x1", h.x);
      guide.setAttribute("x2", h.x);
      while (layer.childNodes.length > 1) layer.removeChild(layer.lastChild);
      tip.textContent = "";
      tip.appendChild(el("div", "when", h.m));
      h.r.forEach(function (r) {
        var line = keyed(b.legend[r[0]]);
        var dot = mark(svg, "circle", "dot " + line[0], { cx: h.x, cy: r[1], r: 3.5 });
        layer.appendChild(dot);
        var item = entry("div", line);
        item.appendChild(el("span", "fig", r[2]));
        tip.appendChild(item);
      });
      // Beside the guide on whichever side holds it: a fixed midpoint would
      // let a box of long names run off a phone's edge and scroll the page.
      var left = (h.x - box.x) / box.width;
      var room = rect.width * (1 - left) - 10;
      tip.style.setProperty("--at", left * 100 + "%");
      tip.classList.toggle("flip", tip.offsetWidth > room && rect.width * left > room);
    }

    plot.addEventListener("pointerdown", show);
    plot.addEventListener("pointermove", show);
    plot.addEventListener("pointerleave", function (e) {
      if (e.pointerType === "mouse") hide();
    });
    document.addEventListener("pointerdown", function (e) {
      if (!plot.contains(e.target)) hide();
    });
  }

  function chart(b) {
    var wrap = el("div", "chart");
    wrap.appendChild(el("h4", null, b.title));
    var plot = el("div", "plot");
    plot.innerHTML = b.svg;
    wrap.appendChild(plot);
    readout(plot, b);
    wrap.appendChild(legend(b.legend.map(keyed)));
    return wrap;
  }

  function block(b) {
    switch (b.t) {
      case "Heading":
        return fill(el("h3"), b.text);
      case "Subheading":
        return el("h4", null, b.text);
      case "Note":
        return el("p", "note", b.text);
      case "Table":
        return table(b.class, b.head, b.rows);
      case "Ledger":
        return ledger(b);
      case "Bar":
        return bar(b);
      case "Chart":
        return chart(b);
    }
    return el("div");
  }

  // A heading opens a group that holds what follows it up to the next one,
  // so Savings can set its containers side by side.
  function draw(tab) {
    var panel = document.getElementById(tab.id + "-panel");
    if (!panel) return;
    var into = panel;
    var groups = el("div", "groups");
    tab.blocks.forEach(function (b) {
      if (b.t === "Heading") {
        into = el("div", "group");
        groups.appendChild(into);
      }
      into.appendChild(block(b));
    });
    if (groups.childNodes.length) panel.appendChild(groups);
  }

  D.tabs.forEach(draw);

  // One dialog, filled from `histories` by the index a goal's button carries.
  var dialog = el("dialog");
  dialog.id = "hist";
  var head = el("header");
  var title = el("h3");
  title.id = "hist-title";
  dialog.setAttribute("aria-labelledby", "hist-title");
  var close = el("button", "x", "×");
  close.type = "button";
  close.setAttribute("aria-label", "Close");
  head.appendChild(title);
  head.appendChild(close);
  var body = el("div", "body");
  dialog.appendChild(head);
  dialog.appendChild(body);
  document.body.appendChild(dialog);
  close.addEventListener("click", function () {
    dialog.close();
  });
  // The dialog's own box is only ever hit outside its content, which is the
  // backdrop.
  dialog.addEventListener("click", function (e) {
    if (e.target === dialog) dialog.close();
  });
  document.addEventListener("click", function (e) {
    var b = e.target.closest("button.goal");
    if (!b) return;
    var h = D.histories[+b.dataset.h];
    title.textContent = h.name;
    body.textContent = "";
    if (h.note) body.appendChild(el("p", "note", h.note));
    if (h.empty) body.appendChild(el("p", "note", h.empty));
    else body.appendChild(table("hs", [], h.rows));
    dialog.showModal();
    body.scrollTop = 0;
  });
})();
