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
});
