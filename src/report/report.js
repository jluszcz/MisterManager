// Draws the report's payload into the panels the shell left empty.
//
// Every decision is already in the payload: a cell's tone, an account's
// class, a bar's widths, a chart's coordinates. What is here is layout only.
// Owner text goes in through `textContent` and nothing else; the one
// `innerHTML` is a chart's SVG, which Rust built and escaped.
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
  function legend(items) {
    var p = el("p", "legend");
    items.forEach(function (entry) {
      var item = el("span", "item");
      if (entry[0]) item.classList.add(entry[0]);
      entry[1].forEach(function (part) {
        item.appendChild(part);
      });
      p.appendChild(item);
    });
    return p;
  }

  // A legend entry is an account or the Total, each a swatch in the entry's
  // own `currentColor` beside its name.
  function chart(b) {
    var wrap = el("div", "chart");
    wrap.appendChild(el("h4", null, b.title));
    var svg = el("div");
    svg.innerHTML = b.svg;
    wrap.appendChild(svg);
    wrap.appendChild(legend(b.legend.map(function (c) {
      var name = fill(el("span"), c);
      var tone = name.className;
      name.className = "";
      return [tone, [el("span", "key"), name]];
    })));
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
