.pragma library

// One card for each computer.
//
// A computer can be listed in more than one way: as a Lunaris computer (a
// host), and as sources (the same machine reached with RustDesk, with
// Sunshine, ...). A `group` (an opaque word) says which are one computer:
// sources with the same group are one computer, and the group `host:<id>`
// is the Lunaris computer with that id. This file turns the two lists into
// cards, each with its ways to connect ("methods").
//
// Nothing here widens what a person may use: the lists are the ones this
// person was given, and a source joins a Lunaris computer's card only when
// that computer is in the list too.
//
// The web client makes its cards the same way (web/src/computerCards.ts is
// this file's twin: the same rules and the same keys); change neither alone.

var HOST_GROUP = "host:";
var MAX_GROUP = 64;

// Whether `group` can be a group (as the server and the agents check it).
function validGroup(group) {
    return typeof group === "string" && group.length > 0 && group.length <= MAX_GROUP && /^[A-Za-z0-9._:-]+$/.test(group);
}

// A new group for sources that are to be one computer.
function newGroup() {
    var hex = "";
    for (var i = 0; i < 12; i++) hex += Math.floor(Math.random() * 16).toString(16);
    return "g-" + hex;
}

// "RustDesk (by IP address)" reads "RustDesk" on a chip.
function shortTypeName(name) {
    var text = String(name || "");
    return text.replace(/\s*[(（].*$/, "").trim() || text;
}

// The types whose short name is not their name without its brackets.
var SHORT_NAMES = { lunaris: "Lunaris", rustdesk: "RustDesk", sunshine: "Sunshine", rdp: "RDP", vnc: "VNC" };

// The name of a type where there is little room (a tile, a chip): by its
// connector's id for the types that have a name of their own, else the
// connector's name without its brackets. The full name stays for a tool tip.
function typeShortName(connector, name) {
    var id = String(connector || "");
    if (Object.prototype.hasOwnProperty.call(SHORT_NAMES, id)) return SHORT_NAMES[id];
    return shortTypeName(name || id);
}

// The types that have a logo of their own (TypeLogo.qml), by connector id.
var LOGOS = { lunaris: "logos/lunaris.svg", rustdesk: "logos/rustdesk.svg", sunshine: "logos/sunshine.svg",
              rdp: "logos/rdp.svg", vnc: "logos/vnc.svg" };
function typeLogo(connector) {
    var id = String(connector || "");
    return Object.prototype.hasOwnProperty.call(LOGOS, id) ? LOGOS[id] : "";
}
// The type of a way to connect ({ host | source }): "lunaris" or its connector's id.
function methodType(method) {
    return method && method.source ? String(method.source.connector || "") : "lunaris";
}

// ---- The form of a type: which fields share a row ----

// A field that needs little room: a port, or one whose usual value is short.
function shortField(field) {
    var usual = String(field["default"] || "");
    return field.kind === "port" || (field.kind === "text" && usual !== "" && usual.length <= 12);
}

// A choice shown as a drop-down rather than as options to click: many
// options, or one with a long name.
function choiceAsList(field) {
    var options = field && field.kind === "choice" && field.options instanceof Array ? field.options : [];
    if (options.length === 0) return false;
    var longest = 0;
    for (var i = 0; i < options.length; i++) longest = Math.max(longest, String(options[i].label || "").length);
    return options.length > 3 || longest > 32;
}

// A field that may share its row with another: anything but options to
// click whose names together are long (they want the whole row).
function pairable(field) {
    var options = field.kind === "choice" && field.options instanceof Array ? field.options : [];
    if (options.length === 0 || choiceAsList(field)) return true;
    var total = 0;
    for (var i = 0; i < options.length; i++) total += String(options[i].label || "").length;
    return total <= 20;
}

// The fields of a form on a wide window, in rows of 6 columns: a field and
// the port after it share a row (4 + 2); short fields that follow one
// another share rows, three to a row (2 + 2 + 2) and two (3 + 3) where three
// would leave one alone; the fields left with a row of their own then share
// one two by two (3 + 3), in their order (options to click with long names
// keep a row of their own). [{ field, span }], in the fields' order. (On a
// narrow window every field has its own row.)
function layoutFields(fields) {
    var placed = placeFields(fields);
    for (var j = 0; j + 1 < placed.length; j++) {
        if (placed[j].span === 6 && placed[j + 1].span === 6 && pairable(placed[j].field) && pairable(placed[j + 1].field)) {
            placed[j].span = 3;
            placed[j + 1].span = 3;
            j++;
        }
    }
    return placed;
}

function placeFields(fields) {
    var placed = [];
    fields = fields || [];
    // A field that takes the port after it onto its row.
    function withPort(i) {
        return !!fields[i] && fields[i].kind !== "port" && fields[i].kind !== "choice"
            && !!fields[i + 1] && fields[i + 1].kind === "port";
    }
    var i = 0;
    while (i < fields.length) {
        if (withPort(i)) {
            placed.push({ field: fields[i], span: 4 }, { field: fields[i + 1], span: 2 });
            i += 2;
            continue;
        }
        var end = i;
        while (end < fields.length && shortField(fields[end]) && !withPort(end)) end++;
        var run = end - i;
        if (run < 2) {
            placed.push({ field: fields[i], span: 6 });
            i++;
            continue;
        }
        // Rows of three; a rest of one takes one from the row before it (two rows of two).
        var left = run;
        while (left > 0) {
            var row = left === 4 || left === 2 ? 2 : 3;
            for (var k = 0; k < row; k++) placed.push({ field: fields[i++], span: row === 3 ? 2 : 3 });
            left -= row;
        }
    }
    return placed;
}

function sourceKey(source) {
    return "source:" + source.host_id + ":" + source.id;
}

function byName(a, b) {
    return String(a.name).localeCompare(String(b.name)) || String(a.id).localeCompare(String(b.id))
        || String(a.host_id).localeCompare(String(b.host_id));
}

function sourceMethod(source, label) {
    return { key: sourceKey(source), source: source, label: label(source), online: !!source.online };
}

// The cards: the Lunaris computers in the order given, each with the
// sources that are that computer; then the other sources by name, those of
// one group on one card (named after the first of them).
//
// `label(source)` names a source's method (its type); `hostGroup(host)` is
// the group a Lunaris computer is (left out: "host:<its id>").
// A card: { key, title, host (or undefined), group, methods: [{ key, label,
// online, host | source }] }.
function buildCards(hosts, sources, label, lunarisLabel, hostGroup) {
    label = label || function (source) { return typeShortName(source.connector, source.connector_name); };
    lunarisLabel = lunarisLabel || "Lunaris";
    hostGroup = hostGroup || function (host) { return HOST_GROUP + host.id; };
    var sorted = (sources || []).slice().sort(byName);
    function groupOf(source) { return validGroup(source.group) ? source.group : ""; }
    var cards = [];
    var taken = [];
    var i, k;

    for (i = 0; i < (hosts || []).length; i++) {
        var host = hosts[i];
        var group = hostGroup(host);
        // A computer whose id cannot be part of a group takes no methods.
        if (!validGroup(group)) group = "";
        var methods = [{ key: "lunaris", host: host, label: lunarisLabel, online: host.status === "Online" }];
        for (k = 0; k < sorted.length; k++) {
            if (group !== "" && taken.indexOf(sorted[k]) === -1 && groupOf(sorted[k]) === group) {
                taken.push(sorted[k]);
                methods.push(sourceMethod(sorted[k], label));
            }
        }
        cards.push({ key: "host:" + host.id, title: host.name, host: host, group: group, methods: methods });
    }

    var byGroup = {};
    for (k = 0; k < sorted.length; k++) {
        var source = sorted[k];
        if (taken.indexOf(source) !== -1) continue;
        var g = groupOf(source);
        if (g !== "" && byGroup[g]) {
            byGroup[g].methods.push(sourceMethod(source, label));
            continue;
        }
        var card = { key: g !== "" ? "group:" + g : sourceKey(source), title: source.name, host: undefined, group: g, methods: [sourceMethod(source, label)] };
        if (g !== "") byGroup[g] = card;
        cards.push(card);
    }

    // Two methods of one type on a card are told apart by their names.
    for (i = 0; i < cards.length; i++) {
        var count = {};
        var ms = cards[i].methods;
        for (k = 0; k < ms.length; k++) if (ms[k].source) count[ms[k].label] = (count[ms[k].label] || 0) + 1;
        for (k = 0; k < ms.length; k++) if (ms[k].source && count[ms[k].label] > 1) ms[k].label = ms[k].label + ": " + ms[k].source.name;
    }
    return cards;
}

// The method a card connects with: the one remembered for it when it is
// still there and can be connected to; else the Lunaris computer itself, or
// the first method, when that can be; else the first that can be; else (the
// whole card is offline) the remembered one or the first.
function currentMethod(methods, remembered) {
    if (!methods || methods.length === 0) return undefined;
    var kept;
    var i;
    if (remembered) for (i = 0; i < methods.length; i++) if (methods[i].key === remembered) { kept = methods[i]; break; }
    if (kept && kept.online) return kept;
    var first = methods[0];
    if (!kept && first.online) return first;
    for (i = 0; i < methods.length; i++) if (methods[i].online) return methods[i];
    return kept || first;
}

// The most cards a method is remembered for (the oldest are forgotten).
var MAX_REMEMBERED = 200;

// `known` ({ "<card key>": "<method key>" }) with `method` remembered for `card`.
function remembered(known, card, method) {
    var next = {};
    var keys = Object.keys(known || {}).filter(function (key) { return key !== card && typeof known[key] === "string"; });
    keys = keys.slice(Math.max(0, keys.length - (MAX_REMEMBERED - 1)));
    for (var i = 0; i < keys.length; i++) next[keys[i]] = known[keys[i]];
    next[card] = method;
    return next;
}

// The card of `cards` that has the method `key`, or undefined.
function cardWithMethod(cards, key) {
    for (var i = 0; i < cards.length; i++)
        for (var k = 0; k < cards[i].methods.length; k++)
            if (cards[i].methods[k].key === key) return cards[i];
    return undefined;
}

// Where a way to connect is, to show on its card: a Lunaris computer's own
// addresses (LAN, VPN) as its agent last said them, else the address it was
// added by (without the usual port); a source's host (and port). "" when
// that is not known.
function methodAddress(method) {
    if (!method) return "";
    if (method.source) return typeof method.source.address === "string" ? method.source.address : "";
    var host = method.host || {};
    var own = host.addresses instanceof Array
        ? host.addresses.filter(function (a) { return typeof a === "string" && a !== ""; }) : [];
    if (own.length > 0) return own.join(", ");
    if (typeof host.address === "string" && host.address !== "") return host.address.replace(/:8091$/, "");
    return typeof host.ip_address === "string" ? host.ip_address : "";
}

// ---- Finding a computer: search and the type filter ----
// (web/src/computerCards.ts has the same functions; change neither alone.)

// `text` for comparing: lower case, without accents ("Máy Đức" -> "may duc").
function foldText(text) {
    var s = String(text === undefined || text === null ? "" : text);
    if (typeof s.normalize === "function") s = s.normalize("NFD");
    return s.replace(/[̀-ͯ]/g, "").replace(/[đĐ]/g, "d").toLowerCase();
}

// The types of a card's ways to connect ("lunaris" or a connector's id), each once.
function cardTypes(card) {
    var out = [];
    var methods = (card && card.methods) || [];
    for (var i = 0; i < methods.length; i++) {
        var id = methodType(methods[i]);
        if (id !== "" && out.indexOf(id) === -1) out.push(id);
    }
    return out;
}

// What a search looks in: the card's name, and for each way to connect its
// name, address and the computer it is reached through. Folded (foldText).
function cardSearchText(card) {
    var parts = [card ? card.title : ""];
    var methods = (card && card.methods) || [];
    for (var i = 0; i < methods.length; i++) {
        var m = methods[i];
        parts.push(m.label);
        var item = m.host || m.source || {};
        parts.push(item.name, item.address, item.ip_address, item.host_name);
        if (m.host && typeof item.address === "string") parts.push(item.address.replace(/:8091$/, ""));
        // The computer's own addresses (LAN, VPN), and where a source is.
        if (m.host && m.host.addresses instanceof Array) for (var a = 0; a < m.host.addresses.length; a++) parts.push(m.host.addresses[a]);
        if (m.source) parts.push(m.source.address);
    }
    return foldText(parts.filter(function (p) { return p !== undefined && p !== null && p !== ""; }).join("\n"));
}

// Whether a card is found by `query`: every word of it is somewhere in what
// the search looks in (case and accents aside). An empty query finds all.
function matchesSearch(card, query) {
    var words = foldText(query).split(/\s+/).filter(function (w) { return w !== ""; });
    if (words.length === 0) return true;
    var text = cardSearchText(card);
    for (var i = 0; i < words.length; i++) if (text.indexOf(words[i]) === -1) return false;
    return true;
}

// The types to filter by, in the cards: [{ id, name }], Lunaris first, then
// RustDesk and Sunshine, then any other by name. `nameOf(id, source)` names
// one (left out: Lunaris, else the connector's short name).
var TYPE_ORDER = ["lunaris", "rustdesk", "sunshine"];
function typesOf(cards, nameOf) {
    nameOf = nameOf || function (id, source) {
        return id === "lunaris" ? "Lunaris" : typeShortName(id, source ? source.connector_name : id);
    };
    var found = [];
    var seen = {};
    for (var c = 0; c < (cards || []).length; c++) {
        var methods = cards[c].methods || [];
        for (var k = 0; k < methods.length; k++) {
            var id = methodType(methods[k]);
            if (id === "" || Object.prototype.hasOwnProperty.call(seen, id)) continue;
            seen[id] = true;
            found.push({ id: id, name: String(nameOf(id, methods[k].source) || id) });
        }
    }
    found.sort(function (a, b) {
        var ra = TYPE_ORDER.indexOf(a.id), rb = TYPE_ORDER.indexOf(b.id);
        if (ra === -1) ra = TYPE_ORDER.length;
        if (rb === -1) rb = TYPE_ORDER.length;
        return ra - rb || a.name.localeCompare(b.name) || a.id.localeCompare(b.id);
    });
    return found;
}

// The type filtered by: `chosen` while some card has it, else "" (all).
function effectiveType(types, chosen) {
    chosen = String(chosen || "");
    for (var i = 0; i < (types || []).length; i++) if (types[i].id === chosen) return chosen;
    return "";
}

// The cards found by `query` that have a way to connect of type `type`
// ("" = any), in their order.
function filterCards(cards, query, type) {
    type = String(type || "");
    return (cards || []).filter(function (card) {
        return (type === "" || cardTypes(card).indexOf(type) !== -1) && matchesSearch(card, query);
    });
}
