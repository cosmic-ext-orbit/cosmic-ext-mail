// SPDX-License-Identifier: GPL-3.0-only

(() => {
    const canvas = [24, 24, 27];
    const light = [247, 247, 248];
    const dark = [20, 21, 23];

    const parse = value => {
        const match = /^rgba?\(([^)]+)\)$/.exec(value);
        if (!match) {
            return null;
        }
        const [r, g, b, alpha = 1] = match[1].split(",").map(Number);
        return { rgb: [r, g, b], alpha };
    };
    const composite = (color, backdrop) => color.rgb.map(
        (channel, index) => channel * color.alpha + backdrop[index] * (1 - color.alpha)
    );
    const luminance = rgb => rgb.reduce((sum, channel, index) => {
        const value = channel / 255;
        const linear = value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
        return sum + linear * [0.2126, 0.7152, 0.0722][index];
    }, 0);
    const contrast = (first, second) =>
        (Math.max(first, second) + 0.05) / (Math.min(first, second) + 0.05);
    const css = rgb => `rgb(${rgb.join(", ")})`;

    // Identify text backdrops without reading the text of each subtree repeatedly.
    const hasText = new Set();
    if (document.body) {
        const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
        for (let text = walker.nextNode(); text; text = walker.nextNode()) {
            if (!text.data.trim() || text.parentElement.closest("style, script, svg")) {
                continue;
            }
            for (let node = text.parentElement; node && !hasText.has(node); node = node.parentElement) {
                hasText.add(node);
            }
        }
    }

    const backdrops = new Map();
    const updates = [];
    for (const node of document.querySelectorAll("html, body, body *")) {
        if (node.namespaceURI !== "http://www.w3.org/1999/xhtml" ||
            node.matches("img, picture, video, canvas, style, script")) {
            continue;
        }

        const style = getComputedStyle(node);
        const inherited = backdrops.get(node.parentElement) || canvas;
        const background = parse(style.backgroundColor);
        let backdrop = background ? composite(background, inherited) : inherited;
        const changes = [];

        const darkest = Math.min(...backdrop);
        const brightest = Math.max(...backdrop);
        const nearBlack = brightest <= 27 && brightest - darkest <= 8;
        if (background?.alpha > 0 && (nearBlack || luminance(backdrop) > 0.24)) {
            changes.push(["background-color", css(canvas)]);
            backdrop = canvas;
        }
        backdrops.set(node, backdrop);

        if (hasText.has(node) && style.backgroundImage !== "none") {
            changes.push(["background-image", "none"]);
        }
        if (hasText.has(node) && style.mixBlendMode !== "normal") {
            changes.push(["mix-blend-mode", "normal"]);
        }

        const foreground = parse(style.color);
        if (foreground?.alpha > 0 && (hasText.has(node) || node === document.body)) {
            const surface = luminance(backdrop);
            if (contrast(luminance(composite(foreground, backdrop)), surface) < 4.5) {
                const replacement = contrast(luminance(light), surface) >=
                    contrast(luminance(dark), surface) ? light : dark;
                changes.push(["color", css(replacement)]);
            }
        }
        if (changes.length) {
            updates.push([node, changes]);
        }
    }

    // Apply after collecting styles to avoid forcing a layout for each element.
    for (const [node, changes] of updates) {
        for (const [property, value] of changes) {
            node.style.setProperty(property, value, "important");
        }
    }
    return true;
})()
