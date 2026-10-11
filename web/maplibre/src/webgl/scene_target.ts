import type {Context} from './context.ts';

/**
 * Scenic Roads: the frame drawn into textures, so layers can read its depth.
 *
 * WebGL can't sample the canvas's depth buffer, so while a custom layer asks to read the frame's
 * depth (`CustomLayerInterface.sceneDepth`) the main pass is drawn here, into a colour renderbuffer
 * and a depth-stencil texture of the drawing buffer's size, and its colour is blitted to the canvas
 * once the frame is done. Drawing into a texture while sampling it is a feedback loop (an error in
 * WebGL), so a layer reading the depth draws through `colorOnly`, the same colour without the depth
 * attached, and tests depth in its shader. While no layer asks, none of this exists or runs.
 */
export class SceneTarget {
    context: Context;
    width: number;
    height: number;
    color: WebGLRenderbuffer;
    depthStencil: WebGLTexture;
    /** Colour and depth-stencil: what the frame's layers draw into. */
    framebuffer: WebGLFramebuffer;
    /** The same colour alone: what a layer reading `depthStencil` draws into. */
    colorOnly: WebGLFramebuffer;

    constructor(context: Context, width: number, height: number) {
        const gl = context.gl;
        this.context = context;
        this.width = width;
        this.height = height;

        this.color = gl.createRenderbuffer();
        gl.bindRenderbuffer(gl.RENDERBUFFER, this.color);
        gl.renderbufferStorage(gl.RENDERBUFFER, gl.RGBA8, width, height);
        gl.bindRenderbuffer(gl.RENDERBUFFER, null);
        context.bindRenderbuffer.dirty = true;

        this.depthStencil = gl.createTexture();
        gl.bindTexture(gl.TEXTURE_2D, this.depthStencil);
        gl.texStorage2D(gl.TEXTURE_2D, 1, gl.DEPTH24_STENCIL8, width, height);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
        gl.bindTexture(gl.TEXTURE_2D, null);
        // (MapLibre's texture unit cache doesn't know of this binding.)
        context.bindTexture.dirty = true;

        this.framebuffer = gl.createFramebuffer();
        gl.bindFramebuffer(gl.FRAMEBUFFER, this.framebuffer);
        gl.framebufferRenderbuffer(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.RENDERBUFFER, this.color);
        gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.DEPTH_STENCIL_ATTACHMENT, gl.TEXTURE_2D, this.depthStencil, 0);

        this.colorOnly = gl.createFramebuffer();
        gl.bindFramebuffer(gl.FRAMEBUFFER, this.colorOnly);
        gl.framebufferRenderbuffer(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.RENDERBUFFER, this.color);
        context.bindFramebuffer.dirty = true;
        context.bindFramebuffer.set(null);
    }

    /** Copies the frame's colour to the canvas. */
    present(): void {
        const context = this.context;
        const gl = context.gl;
        gl.bindFramebuffer(gl.READ_FRAMEBUFFER, this.framebuffer);
        gl.bindFramebuffer(gl.DRAW_FRAMEBUFFER, null);
        gl.blitFramebuffer(0, 0, this.width, this.height, 0, 0, this.width, this.height, gl.COLOR_BUFFER_BIT, gl.NEAREST);
        gl.bindFramebuffer(gl.FRAMEBUFFER, null);
        context.bindFramebuffer.dirty = true;
        context.bindFramebuffer.set(null);
    }

    destroy(): void {
        const gl = this.context.gl;
        gl.deleteFramebuffer(this.framebuffer);
        gl.deleteFramebuffer(this.colorOnly);
        gl.deleteRenderbuffer(this.color);
        gl.deleteTexture(this.depthStencil);
    }
}
