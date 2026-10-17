import {describe, test, expect, vi} from 'vitest';
import {Program} from './program.ts';
import {shaders} from '../shaders/shaders.ts';
import {Context, LINK_WAIT_FRAMES} from './context.ts';

const COMPLETION_STATUS_KHR = 0x91B1;

/** A WebGL stand-in: every call answers 0 but the link's status and completion, which `state` sets. */
function fakeContext(async: boolean) {
    const state = {complete: false, linked: true, statusAsked: 0, polls: 0};
    const known = {
        LINK_STATUS: 0x8B82, ACTIVE_ATTRIBUTES: 0x8B89, INVALID_INDEX: 0xFFFFFFFF,
        isContextLost: () => false,
        getProgramParameter: (_p: unknown, pname: number) => {
            if (pname === COMPLETION_STATUS_KHR) {
                state.polls++;
                return state.complete;
            }
            if (pname === 0x8B82) {
                state.statusAsked++;
                return state.linked;
            }
            return 0;
        },
        getUniformLocation: () => ({}),
        getAttribLocation: () => 0,
        getProgramInfoLog: () => 'link failed',
        getShaderParameter: () => true,
    };
    const gl = new Proxy(known, {get: (t, k) => (k in t ? t[k] : vi.fn(() => 0))});
    const context = Object.create(Context.prototype) as Context;
    context.gl = gl as unknown as WebGL2RenderingContext;
    context.asyncPrograms = async ? {COMPLETION_STATUS_KHR} : null;
    context.pendingPrograms = 0;
    context.programFrame = 0;
    context._linkingSeenFrame = -1;
    return {context, state};
}

const make = (context: Context, blocking = false) =>
    new Program(context, shaders.background, null, () => ({}), false, false, shaders.projectionMercator, '', [], blocking);

describe('Program in the background (asyncPrograms)', () => {
    test('without it, the link is waited for as the program is made', () => {
        const {context, state} = fakeContext(false);
        const p = make(context);
        expect(state.statusAsked).toBe(1);
        expect(p.ready(context)).toBe(true);
        expect(context.pendingPrograms).toBe(0);
    });

    test('with it, nothing waits until the link completes; then it is finished once', () => {
        const {context, state} = fakeContext(true);
        const p = make(context);
        expect(state.statusAsked).toBe(0);
        // (not asked in the frame it was made)
        expect(p.ready(context)).toBe(false);
        expect(state.polls).toBe(0);
        context.programFrame++;
        expect(p.ready(context)).toBe(false);
        expect(context.pendingPrograms).toBe(2);
        expect(state.polls).toBe(1);
        expect(state.statusAsked).toBe(0);

        state.complete = true;
        context.programFrame++;
        expect(p.ready(context)).toBe(true);
        expect(p.ready(context)).toBe(true);
        expect(state.statusAsked).toBe(1);
        expect(p.fixedUniforms).toEqual({});
    });

    test('a blocking program is waited for even with it', () => {
        const {context, state} = fakeContext(true);
        make(context, true);
        expect(state.statusAsked).toBe(1);
    });

    test('after a link found not complete, none is asked again in that frame', () => {
        const {context, state} = fakeContext(true);
        const a = make(context), b = make(context);
        context.programFrame++;
        expect(a.ready(context)).toBe(false);
        expect(b.ready(context)).toBe(false);
        expect(state.polls).toBe(1);
        state.complete = true;
        context.programFrame++;
        expect(a.ready(context)).toBe(true);
        expect(b.ready(context)).toBe(true);
        expect(state.polls).toBe(3);
    });

    test('a failed link throws once complete', () => {
        const {context, state} = fakeContext(true);
        const p = make(context);
        context.programFrame++;
        state.complete = true;
        state.linked = false;
        expect(() => p.ready(context)).toThrow('Program failed to link: link failed');
        // (and again at each later draw, never drawing without its uniforms)
        expect(() => p.ready(context)).toThrow('Program failed to link: link failed');
    });

    test('a link still not complete after LINK_WAIT_FRAMES frames is waited for', () => {
        const {context, state} = fakeContext(true);
        const p = make(context);
        context.programFrame += LINK_WAIT_FRAMES - 1;
        expect(p.ready(context)).toBe(false);
        context.programFrame++;
        expect(p.ready(context)).toBe(true);
        expect(state.statusAsked).toBe(1);
    });
});
