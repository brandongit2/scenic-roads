import {describe, beforeEach, afterEach, test, expect, vi} from 'vitest';
import {parseGlyphPbf} from '../style/parse_glyph_pbf.ts';
import {GlyphManager} from './glyph_manager.ts';
import fs from 'fs';
import path from 'path';
import {RequestManager} from '../util/request_manager.ts';
import {fakeServer, type FakeServer} from 'nise';
import {bufferToArrayBuffer} from '../util/test/util.ts';
import TinySDF, {type TinySDFOptions} from '@mapbox/tiny-sdf';

import type {CreateRasterizer, Rasterizer} from './glyph_manager.ts';

describe('GlyphManager', () => {
    const pbf = fs.readFileSync(path.join(__dirname, '../../test/unit/assets/0-255.pbf'));
    const GLYPHS = {};
    for (const glyph of parseGlyphPbf(pbf)) {
        GLYPHS[glyph.id] = glyph;
    }

    const identityTransform = new RequestManager();
    let server: FakeServer;

    beforeEach(() => {
        global.fetch = null;
        server = fakeServer.create({autoRespond: true, autoRespondAfter: 0});
    });

    function char(codePoint: number) {
        return String.fromCodePoint(codePoint);
    }

    function createGlyphManager(
        remoteEnabled: boolean,
        font?: string | false,
        language?: string,
        createRasterizer?: CreateRasterizer
    ): GlyphManager {
        const manager = new GlyphManager(identityTransform, font, language, createRasterizer);
        if (remoteEnabled) {
            manager.setURL('https://localhost/fonts/v1/{fontstack}/{range}.pbf');
        }
        return manager;
    }

    function serveGlyphRanges() {
        server.respondWith(/\.pbf$/, function (request) {
            request.respond(200, undefined, bufferToArrayBuffer(pbf) as unknown as string);
        });
    }

    function glyphRangeRequests() {
        return server.requests.filter(request => request.url.endsWith('.pbf'));
    }

    function fakeRasterizer(draw: (text: string) => any = () => GLYPHS[0]) {
        // A canvas of the size TinySDF makes, measuring every glyph as one font size wide.
        return vi.fn((options: TinySDFOptions, padding: number) => ({
            draw,
            buffer: padding,
            size: options.fontSize + options.buffer * 4,
            ctx: {measureText: () => ({actualBoundingBoxLeft: 0, actualBoundingBoxRight: options.fontSize} as TextMetrics)}
        }) as unknown as Rasterizer);
    }

    afterEach(() => {
        vi.clearAllMocks();
        server.restore();
        delete (document as any).fonts;
    });

    test('GlyphManager shares a 0-255 PBF request between different glyphs', async () => {
        serveGlyphRanges();
        const transformRequest = vi.fn((url: string) => ({url}));
        const manager = new GlyphManager(new RequestManager(transformRequest));
        manager.setURL('https://localhost/fonts/v1/{fontstack}/{range}.pbf');

        const returnedGlyphs = await manager.getGlyphs({'Arial Unicode MS': {default: [char(55), char(56)]}});

        expect(returnedGlyphs['Arial Unicode MS'].default[char(55)].metrics.advance).toBe(12);
        expect(returnedGlyphs['Arial Unicode MS'].default[char(56)].metrics).toEqual(GLYPHS[56].metrics);
        const cachedRange = await manager.getGlyphs({'Arial Unicode MS': {default: ['A']}});
        expect(cachedRange['Arial Unicode MS'].default.A.metrics).toEqual(GLYPHS[65].metrics);
        expect(transformRequest).toHaveBeenCalledExactlyOnceWith(
            'https://localhost/fonts/v1/Arial Unicode MS/0-255.pbf', 'Glyphs');
    });

    test('GlyphManager doesn\'t request twice 0-255 PBF if a glyph is missing', async () => {
        server.respondWith(/\.pbf$/, [200, {}, new ArrayBuffer(0)]);
        const manager = createGlyphManager(true);

        const missing = await manager.getGlyphs({'Arial Unicode MS': {default: [char(0x01)]}});
        expect(missing['Arial Unicode MS'].default[char(0x01)]).toBeNull();
        const next = await manager.getGlyphs({'Arial Unicode MS': {default: [char(0x01), char(0x02), '7']}});
        expect(next['Arial Unicode MS'].default[char(0x02)]).toBeNull();
        expect(glyphRangeRequests()).toHaveLength(1);
    });

    test('GlyphManager draws locally a character its served range lacks, and asks for the range once (Scenic Roads)', async () => {
        serveGlyphRanges();
        const createRasterizer = fakeRasterizer();
        const manager = createGlyphManager(true, undefined, undefined, createRasterizer);

        // The fixture is a PBF of 0-255: none of these is in it. A Han character (no local ideograph
        // family set), an Egyptian hieroglyph outside the basic plane, a Cherokee letter.
        for (const codePoint of [0x5e73, 0x13080, 0x13a0]) {
            const returnedGlyphs = await manager.getGlyphs({'Arial Unicode MS': {default: [char(codePoint)]}});
            expect(returnedGlyphs['Arial Unicode MS'].default[char(codePoint)].metrics.isDoubleResolution).toBe(true);
        }
        await manager.getGlyphs({'Arial Unicode MS': {default: [char(0x13a0), char(0x13a1)]}});
        expect(glyphRangeRequests().filter(request => request.url.endsWith('/4864-5119.pbf'))).toHaveLength(1);
        expect(createRasterizer).toHaveBeenCalledTimes(1);
    });

    test('GlyphManager leaves a character that draws nothing unavailable when its range lacks it (Scenic Roads)', async () => {
        server.respondWith(/\.pbf$/, [200, {}, new ArrayBuffer(0)]);
        const createRasterizer = fakeRasterizer();
        const manager = createGlyphManager(true, undefined, undefined, createRasterizer);

        // A newline, a zero-width space, a line separator, a private-use and an unassigned codepoint.
        const ids = ['\n', '​', ' ', '', '͸'];
        const returnedGlyphs = await manager.getGlyphs({'Arial Unicode MS': {default: ids}});
        for (const id of ids) expect(returnedGlyphs['Arial Unicode MS'].default[id]).toBeNull();
        expect(createRasterizer).not.toHaveBeenCalled();
    });

    test('GlyphManager draws every letter of a script that needs shaping locally, asking for no range (Scenic Roads)', async () => {
        serveGlyphRanges();
        const manager = createGlyphManager(true, undefined, undefined, fakeRasterizer());

        // Single codepoints: Devanagari, Bengali, Tamil, Kannada, Odia, Sinhala, Thai, Lao, Tibetan,
        // Burmese, Khmer, Mongolian.
        const ids = ['द', 'ঢ', 'ச', 'ಬ', 'କ', 'ක', 'ก', 'ວ', 'ལ', 'မ', 'ភ', 'ᠮ'];
        const returnedGlyphs = await manager.getGlyphs({'Noto Sans Regular': {default: ids}});
        for (const id of ids) expect(returnedGlyphs['Noto Sans Regular'].default[id].metrics.isDoubleResolution).toBe(true);
        expect(glyphRangeRequests()).toHaveLength(0);

        // Latin, Greek, Cyrillic, Hebrew, Arabic (joined here through its presentation forms),
        // Armenian, Georgian and Ethiopic stay with the glyphs URL.
        await manager.getGlyphs({'Noto Sans Regular': {default: ['a', 'θ', 'ж', 'ש', 'م', 'ﻣ', 'Ե', 'თ', 'አ']}});
        expect(glyphRangeRequests().map(request => decodeURI(request.url).split('/').pop()).sort()).toEqual(
            ['0-255.pbf', '1024-1279.pbf', '1280-1535.pbf', '1536-1791.pbf', '4096-4351.pbf', '4608-4863.pbf', '65024-65279.pbf', '768-1023.pbf'].sort());
    });

    test('GlyphManager keeps a glyph whose top comes to exactly zero where it is (Scenic Roads)', async () => {
        // 55 px above the baseline at 48 px, Thai `น้ำ`: 55 / 2 - 27.5 = 0.
        const draw = () => ({data: new Uint8ClampedArray(72 * 72), width: 72, height: 72, glyphWidth: 60, glyphHeight: 60, glyphTop: 55, glyphLeft: 0, glyphAdvance: 54});
        const manager = createGlyphManager(true, undefined, undefined, fakeRasterizer(draw));

        const returnedGlyphs = await manager.getGlyphs({'Noto Sans Regular': {default: ['น้ำ']}});
        expect(returnedGlyphs['Noto Sans Regular'].default['น้ำ'].metrics.top).toBe(0);
    });

    test('GlyphManager draws a cluster wider than its canvas on a wider one (Scenic Roads)', async () => {
        // A rasterizer that measures two font sizes a letter: the first canvas, three font sizes
        // wide, holds one letter.
        const made: Array<{buffer: number; drew: string[]}> = [];
        const createRasterizer = vi.fn((options: TinySDFOptions, padding: number) => {
            const canvas = {buffer: options.buffer, drew: [] as string[]};
            made.push(canvas);
            return {
                buffer: padding,
                size: options.fontSize + options.buffer * 4,
                ctx: {measureText: (text: string) => ({actualBoundingBoxLeft: 0, actualBoundingBoxRight: [...text].length * 2 * options.fontSize} as TextMetrics)},
                draw: (text: string) => {
                    canvas.drew.push(text);
                    return GLYPHS[0];
                }
            } as any;
        });
        const manager = createGlyphManager(true, undefined, undefined, createRasterizer);

        await manager.getGlyphs({'Noto Sans Regular': {default: ['ᠮ', 'ᠮᠣᠩ', 'ᠮᠣᠩᠭᠣᠯ', 'ᠮᠣᠩᠭᠣ']}});

        // One letter (2 font sizes) on the first; three (6) on one of 6; six and five (12, 10) share one of 12.
        expect(made.map(canvas => canvas.drew)).toEqual([['ᠮ'], ['ᠮᠣᠩ'], ['ᠮᠣᠩᠭᠣᠯ', 'ᠮᠣᠩᠭᠣ']]);
        expect(made[0].buffer).toBeLessThan(made[1].buffer);
        expect(made[1].buffer).toBeLessThan(made[2].buffer);
    });

    test('GlyphManager does not cache CJK chars that should be rendered locally', async () => {
        serveGlyphRanges();
        const manager = createGlyphManager(true, 'sans-serif');

        //Request char that overlaps Katakana range
        let returnedGlyphs = await manager.getGlyphs({'Arial Unicode MS': {default: [char(0x3005)]}});
        expect(returnedGlyphs['Arial Unicode MS'].default[char(0x3005)]).not.toBeNull();
        //Request char from Katakana range (te テ)
        returnedGlyphs = await manager.getGlyphs({'Arial Unicode MS': {default: [char(0x30C6)]}});
        const glyph = returnedGlyphs['Arial Unicode MS'].default[char(0x30c6)];
        //Ensure that te is locally generated.
        expect(glyph.bitmap.height).toBe(12);
        expect(glyph.bitmap.width).toBe(12);
    });

    test('GlyphManager generates CJK PBF locally', async () => {
        const manager = createGlyphManager(true, 'sans-serif');

        // Chinese character píng 平
        const returnedGlyphs = await manager.getGlyphs({'Arial Unicode MS': {default: [char(0x5e73)]}});
        expect(returnedGlyphs['Arial Unicode MS'].default[char(0x5e73)].metrics.advance).toBe(0.5);
    });

    test('GlyphManager generates non-BMP CJK PBF locally', async () => {
        const manager = createGlyphManager(true, 'sans-serif');

        // Chinese character biáng 𰻞
        const returnedGlyphs = await manager.getGlyphs({'Arial Unicode MS': {default: [char(0x30EDE)]}});
        expect(returnedGlyphs['Arial Unicode MS'].default[char(0x30EDE)].metrics.advance).toBe(1);
    });

    test('GlyphManager generates Katakana PBF locally', async () => {
        const manager = createGlyphManager(true, 'sans-serif');

        // Katakana letter te テ
        const returnedGlyphs = await manager.getGlyphs({'Arial Unicode MS': {default: [char(0x30c6)]}});
        expect(returnedGlyphs['Arial Unicode MS'].default[char(0x30c6)].metrics.advance).toBe(0.5);
    });

    test('GlyphManager generates Hiragana PBF locally', async () => {
        const manager = createGlyphManager(true, 'sans-serif');

        //Hiragana letter te て
        const returnedGlyphs = await manager.getGlyphs({'Arial Unicode MS': {default: [char(0x3066)]}});
        expect(returnedGlyphs['Arial Unicode MS'].default[char(0x3066)].metrics.advance).toBe(0.5);
    });

    test('GlyphManager consistently generates CJKV text locally', async () => {
        const manager = createGlyphManager(true, 'sans-serif');

        // Space
        expect(manager._charUsesLocalIdeographFontFamily(0x0020)).toBe(false);
        // Chinese character píng 平
        expect(manager._charUsesLocalIdeographFontFamily(0x5e73)).toBe(true);
        // Chinese character biáng 𰻞
        expect(manager._charUsesLocalIdeographFontFamily(0x30EDE)).toBe(true);
        // Katakana letter te テ
        expect(manager._charUsesLocalIdeographFontFamily(0x30c6)).toBe(true);
        // Hiragana letter te て
        expect(manager._charUsesLocalIdeographFontFamily(0x3066)).toBe(true);
        // Hangul letter a 아
        expect(manager._charUsesLocalIdeographFontFamily(0xC544)).toBe(true);
        // Japanese full-width dash ー
        expect(manager._charUsesLocalIdeographFontFamily(0x30FC)).toBe(true);
        // Halfwidth and Fullwidth Forms: full-width exclamation ！
        expect(manager._charUsesLocalIdeographFontFamily(0xFF01)).toBe(true);
        // CJK Symbols and Punctuation: Japanese Post mark 〒
        expect(manager._charUsesLocalIdeographFontFamily(0x3012)).toBe(true);
    });

    test('GlyphManager locally generates Latin character', async () => {
        const manager = createGlyphManager(false, 'sans-serif');

        // A
        const returnedGlyphs = await manager.getGlyphs({'Times Old Roman': {default: [char(0x41)]}});
        expect(returnedGlyphs['Times Old Roman'].default[char(0x41)].metrics.width).toBeGreaterThan(0);
        expect(returnedGlyphs['Times Old Roman'].default[char(0x41)].metrics.advance).toBeGreaterThan(0);
    });

    test('GlyphManager locally generates nonspacing control character', async () => {
        const manager = createGlyphManager(false, 'sans-serif');

        // U+202E RIGHT-TO-LEFT OVERRIDE
        const returnedGlyphs = await manager.getGlyphs({'Ctrl Alt Del': {default: [char(0x202e)]}});
        expect(returnedGlyphs['Ctrl Alt Del'].default[char(0x202e)].metrics.width).toBe(0);
        expect(returnedGlyphs['Ctrl Alt Del'].default[char(0x202e)].metrics.advance).toBe(0);
    });

    test('GlyphManager locally generates a grapheme cluster when the style has no glyphs URL', async () => {
        const manager = createGlyphManager(false, 'sans-serif');

        // \u0926\u093f is Devanagari DA with the vowel sign I, which is written as one shape
        const cluster = '\u0926\u093f';
        const returnedGlyphs = await manager.getGlyphs({'Noto Sans': {default: [cluster]}});

        expect(returnedGlyphs['Noto Sans'].default[cluster].metrics.advance).toBeGreaterThan(0);
    });

    test('GlyphManager locally generates a grapheme cluster the glyphs URL has no way to serve', async () => {
        const manager = createGlyphManager(true, 'sans-serif');

        const cluster = '\u0926\u093f';
        const returnedGlyphs = await manager.getGlyphs({'Noto Sans': {default: [cluster]}});

        expect(returnedGlyphs['Noto Sans'].default[cluster].metrics.advance).toBeGreaterThan(0);
    });

    test('GlyphManager matches font styles', async () => {
        const manager = createGlyphManager(false, 'sans-serif');

        expect(manager._fontStyle('Swiss Italic')).toBe('italic');
        expect(manager._fontStyle('Swiss Oblique')).toBe('oblique');
        expect(manager._fontStyle('Swiss Roman')).toBe('normal');
        expect(manager._fontStyle('Swiss Cursive')).toBe('normal');
    });

    test('GlyphManager matches font weights', async () => {
        const manager = createGlyphManager(false, 'sans-serif');

        expect(manager._fontWeight('Swiss Thin')).toBe('100');
        expect(manager._fontWeight('Swiss Regular')).toBe('400');
        expect(manager._fontWeight('Swiss Bold')).toBe('700');
        expect(manager._fontWeight('Swiss Extra Bold')).toBe('800');
        expect(manager._fontWeight('Swiss Cheese')).toBeUndefined();
    });

    test('GlyphManager generates missing PBF locally', async () => {
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        server.respondWith(function (request) { request.respond(404, undefined, 'Not Found'); });
        const manager = createGlyphManager(true, 'sans-serif');

        const returnedGlyphs = await manager.getGlyphs({'Arial Unicode MS': {default: [char(0x10e1)]}});

        expect(returnedGlyphs['Arial Unicode MS'].default[char(0x10e1)].metrics.advance).toBeGreaterThan(0);
        expect(console.warn).toHaveBeenCalledWith(expect.stringContaining('Unable to load glyph range'));
    });

    test('GlyphManager caches locally generated glyphs', async () => {

        const manager = createGlyphManager(true, 'sans-serif');
        const drawSpy = vi.spyOn(TinySDF.prototype, 'draw').mockReturnValue({data: new Uint8ClampedArray(60 * 60)} as any);

        // Katakana letter te
        const returnedGlyphs = await manager.getGlyphs({'Arial Unicode MS': {default: [char(0x30c6)]}});
        expect(returnedGlyphs['Arial Unicode MS'].default[char(0x30c6)].metrics.advance).toBe(24);
        await manager.getGlyphs({'Arial Unicode MS': {default: [char(0x30c6)]}});
        expect(drawSpy).toHaveBeenCalledTimes(1);
    });

    test('GlyphManager draws a glyph once for requests made while it waits for its slice (Scenic Roads)', async () => {
        const manager = createGlyphManager(true, 'sans-serif');
        const drawSpy = vi.spyOn(TinySDF.prototype, 'draw').mockReturnValue({data: new Uint8ClampedArray(60 * 60)} as any);

        // Katakana letter te, asked for twice at once
        const [a, b] = await Promise.all([
            manager.getGlyphs({'Arial Unicode MS': {default: [char(0x30c6)]}}),
            manager.getGlyphs({'Arial Unicode MS': {default: [char(0x30c6)]}}),
        ]);
        expect(a['Arial Unicode MS'].default[char(0x30c6)].metrics.advance).toBe(24);
        expect(b['Arial Unicode MS'].default[char(0x30c6)].metrics.advance).toBe(24);
        expect(drawSpy).toHaveBeenCalledTimes(1);
    });

    test('GlyphManager passes no language to TinySDF by default', async () => {
        const createRasterizer = fakeRasterizer();
        const manager = createGlyphManager(true, 'sans-serif', undefined, createRasterizer);
        await manager.getGlyphs({'Arial Unicode MS': {default: [char(0x30c6)]}});
        expect(createRasterizer).toHaveBeenCalledWith(expect.not.objectContaining({lang: expect.anything()}), expect.any(Number));
    });

    test('GlyphManager sets the language on TinySDF', async () => {
        const createRasterizer = fakeRasterizer();
        const manager = createGlyphManager(true, 'sans-serif', 'zh', createRasterizer);
        await manager.getGlyphs({'Arial Unicode MS': {default: [char(0x30c6)]}});
        expect(createRasterizer).toHaveBeenCalledWith(expect.objectContaining({lang: 'zh'}), expect.any(Number));
    });

    test('awaits document.fonts.load before instantiating TinySDF', async () => {
        const loadSpy = vi.fn(() => Promise.resolve([]));
        Object.defineProperty(document, 'fonts', {configurable: true, value: {load: loadSpy}});
        const createRasterizer = fakeRasterizer();

        const manager = createGlyphManager(false, 'sans-serif', undefined, createRasterizer);
        await manager.getGlyphs({'Arial Unicode MS': {default: [char(0x41)]}});

        expect(loadSpy).toHaveBeenCalledTimes(1);
        expect(createRasterizer).toHaveBeenCalledTimes(1);
        expect(loadSpy.mock.invocationCallOrder[0]).toBeLessThan(createRasterizer.mock.invocationCallOrder[0]);
    });

    test('still instantiates TinySDF when document.fonts.load rejects', async () => {
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        const loadSpy = vi.fn(() => Promise.reject(new Error('font not found')));
        Object.defineProperty(document, 'fonts', {configurable: true, value: {load: loadSpy}});
        const createRasterizer = fakeRasterizer();

        const manager = createGlyphManager(false, 'sans-serif', undefined, createRasterizer);
        const result = await manager.getGlyphs({'Arial Unicode MS': {default: [char(0x41)]}});

        expect(loadSpy).toHaveBeenCalledTimes(1);
        expect(createRasterizer).toHaveBeenCalledTimes(1);
        expect(result['Arial Unicode MS'].default[char(0x41)]).toBeDefined();
    });

    test('memoizes document.fonts.load per fontstack', async () => {
        const loadSpy = vi.fn(() => Promise.resolve([]));
        Object.defineProperty(document, 'fonts', {configurable: true, value: {load: loadSpy}});
        const createRasterizer = fakeRasterizer();

        const manager = createGlyphManager(false, 'sans-serif', undefined, createRasterizer);
        await manager.getGlyphs({'Arial Unicode MS': {default: [char(0x41)]}});
        await manager.getGlyphs({'Arial Unicode MS': {default: [char(0x42)]}});
        await manager.getGlyphs({'Arial Unicode MS': {default: [char(0x43)]}});

        expect(loadSpy).toHaveBeenCalledTimes(1);
    });

    test('keeps unsupported variants separate from default glyphs', async () => {
        serveGlyphRanges();
        const createRasterizer = fakeRasterizer();
        const manager = createGlyphManager(true, undefined, undefined, createRasterizer);

        const alternate = await manager.getGlyphs({Test: {alternate: ['A']}});
        expect(alternate.Test).toEqual({alternate: {A: null}});
        expect(server.requests).toHaveLength(0);
        expect(createRasterizer).not.toHaveBeenCalled();

        const request = {Test: {default: ['A'], alternate: ['A']}};
        const glyphs = await manager.getGlyphs(request);
        expect(glyphs.Test.default.A.metrics).toEqual(GLYPHS[65].metrics);
        expect(glyphs.Test.alternate.A).toBeNull();
        await expect(manager.getGlyphs(request)).resolves.toEqual(glyphs);
        expect(glyphRangeRequests()).toHaveLength(1);
    });

    describe('font-faces', () => {
        function stubFontFaces() {
            Object.defineProperty(document, 'fonts', {
                configurable: true,
                value: {load: () => Promise.resolve([]), add: () => {}, delete: () => {}}
            });
            (globalThis as any).FontFace = class {
                family: string;
                constructor(family: string) {
                    this.family = family;
                }
                load = () => Promise.resolve(this);
            };
            server.respondWith(/\.ttf$/, function (request) { request.respond(200, undefined, 'font file'); });
        }

        afterEach(() => {
            delete (globalThis as any).FontFace;
        });

        test('draws a covered codepoint with the declared font file instead of downloading a range', async () => {
            stubFontFaces();
            serveGlyphRanges();
            const createRasterizer = fakeRasterizer();

            const manager = createGlyphManager(true, undefined, undefined, createRasterizer);
            manager.setFontFaces({'Arial Unicode MS': [{url: 'https://localhost/khmer.ttf', 'unicode-range': ['U+1780-17FF']}]});

            const returnedGlyphs = await manager.getGlyphs({'Arial Unicode MS': {default: [char(0x1780)]}});

            expect(returnedGlyphs['Arial Unicode MS'].default[char(0x1780)]).toBeDefined();
            expect(glyphRangeRequests()).toHaveLength(0);
            expect(createRasterizer).toHaveBeenCalledWith(expect.objectContaining({
                fontFamily: expect.stringMatching(/^maplibre-gl-font-face-\d+,sans-serif$/)
            }), expect.any(Number));
        });

        test('draws a grapheme cluster as one glyph, from the file covering the letter it starts with', async () => {
            stubFontFaces();
            serveGlyphRanges();
            const drawn: string[] = [];
            const createRasterizer = fakeRasterizer((text) => { drawn.push(text); return GLYPHS[0]; });

            const manager = createGlyphManager(true, undefined, undefined, createRasterizer);
            manager.setFontFaces({'Arial Unicode MS': 'https://localhost/hebrew.ttf'});

            const shinWithShevaAndDot = '\u05E9\u05B0\u05C1';
            const returnedGlyphs = await manager.getGlyphs({'Arial Unicode MS': {default: [shinWithShevaAndDot]}});

            expect(returnedGlyphs['Arial Unicode MS'].default[shinWithShevaAndDot]).toBeDefined();
            expect(drawn).toContain(shinWithShevaAndDot);
            expect(glyphRangeRequests()).toHaveLength(0);
        });

        test('draws an uncovered cluster from local fonts', async () => {
            stubFontFaces();
            serveGlyphRanges();
            const createRasterizer = fakeRasterizer();

            const manager = createGlyphManager(true, undefined, undefined, createRasterizer);
            const khmerOnly = [{url: 'https://localhost/khmer.ttf', 'unicode-range': ['U+1780-17FF']}];
            manager.setFontFaces({'Arial Unicode MS': khmerOnly});

            const shinWithShevaAndDot = '\u05E9\u05B0\u05C1';
            const returnedGlyphs = await manager.getGlyphs({'Arial Unicode MS': {
                default: [shinWithShevaAndDot]
            }});

            expect(returnedGlyphs['Arial Unicode MS'].default[shinWithShevaAndDot]).not.toBeNull();
            expect(createRasterizer).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({
                fontFamily: 'Arial Unicode MS,sans-serif'
            }), expect.any(Number));
            expect(server.requests).toHaveLength(0);
        });

        test('leaves a codepoint outside every declared range to the glyphs URL', async () => {
            stubFontFaces();
            serveGlyphRanges();

            const manager = createGlyphManager(true);
            manager.setFontFaces({'Arial Unicode MS': [{url: 'https://localhost/khmer.ttf', 'unicode-range': ['U+1780-17FF']}]});

            const returnedGlyphs = await manager.getGlyphs({'Arial Unicode MS': {default: [char(55)]}});

            expect(returnedGlyphs['Arial Unicode MS'].default[char(55)].metrics.advance).toBe(12);
            expect(glyphRangeRequests()).toHaveLength(1);
        });

        test('does not sniff a weight or a style out of the font name, which the file already carries', async () => {
            stubFontFaces();
            const createRasterizer = fakeRasterizer();

            const manager = createGlyphManager(false, undefined, undefined, createRasterizer);
            manager.setFontFaces({'Noto Sans Bold Italic': 'https://localhost/noto-bold-italic.ttf'});
            await manager.getGlyphs({'Noto Sans Bold Italic': {default: [char(0x41)]}});

            expect(createRasterizer).toHaveBeenCalledWith(expect.objectContaining({fontWeight: undefined, fontStyle: 'normal'}), expect.any(Number));
        });

        test('keeps one TinySDF per declared file so a fallback cannot bleed into the rest of the text', async () => {
            stubFontFaces();
            const createRasterizer = fakeRasterizer();

            const manager = createGlyphManager(false, undefined, undefined, createRasterizer);
            manager.setFontFaces({'Arial Unicode MS': [
                {url: 'https://localhost/khmer.ttf', 'unicode-range': ['U+1780-17FF']},
                {url: 'https://localhost/devanagari.ttf', 'unicode-range': ['U+0900-097F']}
            ]});

            await manager.getGlyphs({'Arial Unicode MS': {default: [char(0x1780), char(0x1781), char(0x0915)]}});

            const families = createRasterizer.mock.calls.map(([options]) => options.fontFamily);
            expect(new Set(families).size).toBe(2);
        });

        test('redraws with the new font faces rather than serving the glyphs cached from the old ones', async () => {
            stubFontFaces();
            const createRasterizer = fakeRasterizer();

            const manager = createGlyphManager(false, undefined, undefined, createRasterizer);
            manager.setFontFaces({'Arial Unicode MS': 'https://localhost/noto.ttf'});
            await manager.getGlyphs({'Arial Unicode MS': {default: [char(0x41)]}});

            manager.setFontFaces({'Arial Unicode MS': 'https://localhost/other.ttf'});
            await manager.getGlyphs({'Arial Unicode MS': {default: [char(0x41)]}});

            const families = createRasterizer.mock.calls.map(([options]) => options.fontFamily);
            expect(families).toHaveLength(2);
            expect(families[0]).not.toBe(families[1]);
        });

        test('gives a Burmese syllable, drawn nearly twice as wide as one character, a canvas it is not cut off by', async () => {
            stubFontFaces();
            const createRasterizer = fakeRasterizer();

            const manager = createGlyphManager(false, undefined, undefined, createRasterizer);
            manager.setFontFaces({'Arial Unicode MS': 'https://localhost/myanmar.ttf'});

            await manager.getGlyphs({'Arial Unicode MS': {default: ['\u101C\u102C\u1038', char(0x41)]}});

            const buffers = createRasterizer.mock.calls.map(([options]) => options.buffer);
            expect(Math.max(...buffers)).toBeGreaterThan(Math.min(...buffers));
        });
    });
});
