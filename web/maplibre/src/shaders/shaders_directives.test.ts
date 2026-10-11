// Scenic Roads: the shaders as build/shaders_plugin.ts minifies them. Its minifier joins a line that
// ends in ';', '(', ')', ',', '{' or '}' with the next unless that one is a directive, so a directive
// ending in ')' (`#if defined(A) && !defined(B)`) swallowed the statement after it and the program
// failed to compile in the browser alone: no shader may lose a directive's line to it.
import {describe, test, expect} from 'vitest';
import {shaders} from './shaders.ts';

describe('the minified shaders\' preprocessor directives', () => {
    for (const [name, shader] of Object.entries(shaders)) {
        test(name, () => {
            for (const source of [shader.vertexSource, shader.fragmentSource]) {
                for (const line of source.split('\n')) {
                    if (!line.startsWith('#')) continue;
                    expect(line, `${name}: a directive with a statement joined to it`).not.toMatch(/[;{}]/);
                }
            }
        });
    }

    // The lifted layers' program variant (painter.useProgram's LIFTED define): the flat programs
    // as upstream's, the lifted ones clipping to their tile, the globe's polar caps inside it.
    test('the fill programs map the polar caps\' vertices onto their tile\'s edge in the LIFTED variant', () => {
        for (const name of ['fill', 'fillOutline', 'fillPattern', 'fillOutlinePattern']) {
            const v = shaders[name].vertexSource;
            const start = v.indexOf('#ifdef LIFTED\nfloat ele');
            const lifted = v.slice(start, v.indexOf('#else', start)).replace(/ /g, '');
            expect(lifted, name).toContain('if(a_pos.y<-32767.5)v_tile_pos.y=0.0;');
            expect(lifted, name).toContain('if(a_pos.y>32766.5)v_tile_pos.y=8192.0;');
        }
    });
});
