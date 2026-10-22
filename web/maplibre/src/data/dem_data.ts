import {RGBAImage} from '../util/image.ts';
import {warnOnce} from '../util/util.ts';
import {register} from '../util/web_worker_transfer.ts';

/**
 * The possible DEM encoding types
 */
export type DEMEncoding = 'mapbox' | 'terrarium' | 'custom';

/**
 * DEMData is a data structure for decoding, backfilling, and storing elevation data for processing in the hillshade shaders
 * data can be populated either from a png raw image tile or from serialized data sent back from a worker. When data is initially
 * loaded from a image tile, we decode the pixel values using the appropriate decoding formula, but we store the
 * elevation data as an Int32 value. we add 65536 (2^16) to eliminate negative values and enable the use of
 * integer overflow when creating the texture used in the hillshadePrepare step.
 *
 * DEMData also handles the backfilling of data from a tile's neighboring tiles (`backfillFrom`). This is necessary because we use a pixel's 8
 * surrounding pixel values to compute the slope at that pixel, and we cannot accurately calculate the slope at pixels on a
 * tile's edge without backfilling from neighboring tiles.
 */
/** Where each grid of `DEMData.gridMin` and `gridMax` starts, finest first, and their length. */
const GRID = 32;
const GRID_OFFSETS: number[] = [];
let GRID_LENGTH = 0;
for (let n = GRID; n >= 1; n >>= 1) {
    GRID_OFFSETS.push(GRID_LENGTH);
    GRID_LENGTH += n * n;
}

export class DEMData {
    private static readonly byteViewCache = new WeakMap<DEMData, Uint8Array>();
    /** The cells across the tile in the finest grid of `gridMin` and `gridMax`. */
    static readonly GRID: number = GRID;

    uid: string | number;
    data: Uint32Array;
    stride: number;
    dim: number;
    min: number;
    max: number;
    /**
     * Scenic Roads: the minimum and maximum of each cell of grids over the tile, `DEMData.GRID`
     * cells across and then each half as fine down to one (row-major, north row first, finest grid
     * first), so the elevation range of any part of the tile is known from a few cells without
     * reading its pixels (`getMinMaxInRegion`).
     */
    gridMin: Float32Array;
    gridMax: Float32Array;
    redFactor: number;
    greenFactor: number;
    blueFactor: number;
    baseShift: number;

    /**
     * Constructs a `DEMData` object
     * @param uid - the tile's unique id
     * @param data - RGBAImage data has uniform 2px padding on all sides: square tile edge size defines stride
    // and dim is calculated as stride - 4.
     * @param encoding - the encoding type of the data
     * @param redFactor - the red channel factor used to unpack the data, used for `custom` encoding only
     * @param greenFactor - the green channel factor used to unpack the data, used for `custom` encoding only
     * @param blueFactor - the blue channel factor used to unpack the data, used for `custom` encoding only
     * @param baseShift - the base shift used to unpack the data, used for `custom` encoding only
     */
    constructor(uid: string | number, data: RGBAImage | ImageData, encoding: DEMEncoding, redFactor = 1.0, greenFactor = 1.0, blueFactor = 1.0, baseShift = 0.0) {
        this.uid = uid;
        if (data.height !== data.width) throw new RangeError('DEM tiles must be square');
        if (encoding && !['mapbox', 'terrarium', 'custom'].includes(encoding)) {
            warnOnce(`"${encoding}" is not a valid encoding type. Valid types include "mapbox", "terrarium" and "custom".`);
            return;
        }
        this.stride = data.height;
        const dim = this.dim = data.height - 4;
        this.data = new Uint32Array(data.data.buffer);
        DEMData.byteViewCache.set(this, new Uint8Array(this.data.buffer));
        switch (encoding) {
            case 'terrarium':
                // unpacking formula for mapzen terrarium:
                // https://aws.amazon.com/public-datasets/terrain/
                this.redFactor = 256.0;
                this.greenFactor = 1.0;
                this.blueFactor = 1.0 / 256.0;
                this.baseShift = 32768.0;
                break;
            case 'custom':
                this.redFactor = redFactor;
                this.greenFactor = greenFactor;
                this.blueFactor = blueFactor;
                this.baseShift = baseShift;
                break;
            case 'mapbox':
            default:
                // unpacking formula for mapbox.terrain-rgb:
                // https://www.mapbox.com/help/access-elevation-data/#mapbox-terrain-rgb
                this.redFactor = 6553.6;
                this.greenFactor = 25.6;
                this.blueFactor = 0.1;
                this.baseShift = 10000.0;
                break;
        }

        // in order to avoid flashing seams between tiles, here we are initially populating a 2px border of pixels around the image
        // with the data of the nearest pixel from the image. this data is eventually replaced when the tile's neighboring
        // tiles are loaded and the accurate data can be backfilled using DEMData#backfillFrom
        for (let y = -2; y < dim + 2; y++) {
            const offset = this._idx(0, Math.max(0, Math.min(dim - 1, y)));
            if (y < 0 || y >= dim) this.data.copyWithin(this._idx(0, y), offset, offset + dim);
            this.data.fill(this.data[offset], this._idx(-2, y), this._idx(0, y));
            this.data.fill(this.data[offset + dim - 1], this._idx(dim, y), this._idx(dim + 1, y) + 1);
        }

        // calculate min/max values, the tile's and each grid cell's
        const pixels = this._getByteView();
        this.min = Number.MAX_SAFE_INTEGER;
        this.max = Number.MIN_SAFE_INTEGER;
        const G = DEMData.GRID;
        const gridMin = this.gridMin = new Float32Array(GRID_LENGTH).fill(Infinity);
        const gridMax = this.gridMax = new Float32Array(GRID_LENGTH).fill(-Infinity);
        for (let x = 0; x < dim; x++) {
            const gx = Math.floor(x * G / dim);
            for (let y = 0; y < dim; y++) {
                const index = this._idx(x, y) * 4;
                const ele = this._unpackAtIndex(pixels, index);
                if (ele > this.max) this.max = ele;
                if (ele < this.min) this.min = ele;
                const cell = Math.floor(y * G / dim) * G + gx;
                if (ele > gridMax[cell]) gridMax[cell] = ele;
                if (ele < gridMin[cell]) gridMin[cell] = ele;
            }
        }
        if (dim < G) {
            // (a tile of fewer pixels than cells: each cell takes the pixels it overlaps)
            for (let cy = 0; cy < G; cy++) {
                for (let cx = 0; cx < G; cx++) {
                    for (let y = Math.floor(cy * dim / G); y < Math.ceil((cy + 1) * dim / G); y++) {
                        for (let x = Math.floor(cx * dim / G); x < Math.ceil((cx + 1) * dim / G); x++) {
                            const ele = this._unpackAtIndex(pixels, this._idx(x, y) * 4);
                            gridMin[cy * G + cx] = Math.min(gridMin[cy * G + cx], ele);
                            gridMax[cy * G + cx] = Math.max(gridMax[cy * G + cx], ele);
                        }
                    }
                }
            }
        }
        // the coarser grids, each cell its four children's
        for (let level = 1, n = G >> 1; n >= 1; level++, n >>= 1) {
            const at = GRID_OFFSETS[level], from = GRID_OFFSETS[level - 1];
            for (let y = 0; y < n; y++) {
                for (let x = 0; x < n; x++) {
                    const c = from + 2 * y * 2 * n + 2 * x;
                    gridMin[at + y * n + x] = Math.min(gridMin[c], gridMin[c + 1], gridMin[c + 2 * n], gridMin[c + 2 * n + 1]);
                    gridMax[at + y * n + x] = Math.max(gridMax[c], gridMax[c + 1], gridMax[c + 2 * n], gridMax[c + 2 * n + 1]);
                }
            }
        }
    }

    /**
     * Scenic Roads: the elevation range of a part of the tile, from the grid: the cells it touches,
     * with a pixel's margin (the terrain's mesh interpolates between pixel centres, so its surface at
     * the region's edge reaches the next pixel). Coordinates are fractions of the tile, 0 at its
     * west or north edge. Falls back to the whole tile's range for data made without the grid.
     */
    getMinMaxInRegion(x0: number, y0: number, x1: number, y1: number): {min: number; max: number} {
        if (!this.gridMin) return {min: this.min, max: this.max};
        const margin = 1 / this.dim;
        x0 -= margin; y0 -= margin; x1 += margin; y1 += margin;
        // the finest grid on which the region spans at most four cells' width: 5 × 5 cells at most
        const size = Math.max(x1 - x0, y1 - y0);
        let level = 0, n = DEMData.GRID;
        while (n > 1 && size > 4 / n) { level++; n >>= 1; }
        const cell = (v: number) => Math.min(n - 1, Math.max(0, Math.floor(v * n)));
        const cx0 = cell(x0), cx1 = cell(x1), cy0 = cell(y0), cy1 = cell(y1);
        const at = GRID_OFFSETS[level];
        let min = Infinity, max = -Infinity;
        for (let cy = cy0; cy <= cy1; cy++) {
            for (let cx = cx0; cx <= cx1; cx++) {
                const i = at + cy * n + cx;
                if (this.gridMin[i] < min) min = this.gridMin[i];
                if (this.gridMax[i] > max) max = this.gridMax[i];
            }
        }
        return {min, max};
    }

    get(x: number, y: number): number {
        const pixels = this._getByteView();
        const index = this._idx(x, y) * 4;
        return this._unpackAtIndex(pixels, index);
    }

    sampleBilinear(x: number, y: number): number {
        const cx = Math.floor(x);
        const cy = Math.floor(y);
        if (cx < -1 || cx >= this.dim || cy < -1 || cy >= this.dim) throw new RangeError(`Out of range source coordinates for DEM data. x: ${x}, y: ${y}, dim: ${this.dim}`);

        const pixels = this._getByteView();
        const index = ((cy + 2) * this.stride + cx + 2) * 4;
        const strideByteWidth = this.stride * 4;
        const tx = x - cx;
        const ty = y - cy;
        const z00 = this._unpackAtIndex(pixels, index);
        const z10 = this._unpackAtIndex(pixels, index + 4);
        const z01 = this._unpackAtIndex(pixels, index + strideByteWidth);
        const z11 = this._unpackAtIndex(pixels, index + strideByteWidth + 4);

        return (
            z00 * (1 - tx) * (1 - ty) +
            z10 * tx * (1 - ty) +
            z01 * (1 - tx) * ty +
            z11 * tx * ty
        );
    }

    getUnpackVector(): number[] {
        return [this.redFactor, this.greenFactor, this.blueFactor, this.baseShift];
    }

    _idx(x: number, y: number): number {
        if (x < -2 || x >= this.dim + 2 ||  y < -2 || y >= this.dim + 2) throw new RangeError(`Out of range source coordinates for DEM data. x: ${x}, y: ${y}, dim: ${this.dim}`);
        return (y + 2) * this.stride + (x + 2);
    }

    unpack(r: number, g: number, b: number): number {
        return (r * this.redFactor + g * this.greenFactor + b * this.blueFactor - this.baseShift);
    }

    pack(v: number): {r: number; g: number; b: number} {
        return packDEMData(v, this.getUnpackVector());
    }

    getPixels(): RGBAImage {
        return new RGBAImage({width: this.stride, height: this.stride}, this._getByteView());
    }

    /**
     * Scenic Roads: fills the part of this tile's 2 px border that `other` covers, each pixel from
     * `other`'s pixel holding its centre, as stored (exact between tiles of one zoom; a source whose
     * pixels pack more than a height, the slope's four slopes or the trees' classes, kept whole).
     * `other` covers [ox, ox + size) × [oy, oy + size) in units of this tile (0 to 1 west to east
     * and north to south), at any zoom: a finer or coarser neighbour, so the hill-shading and the
     * terrain meet across an edge between zooms (`scenic:zoom-by-area`). Replaces upstream's
     * `backfillBorder`, which took same-zoom neighbours only.
     */
    backfillFrom(other: DEMData, ox: number, oy: number, size: number): void {
        const dim = this.dim, odim = other.dim;
        const at = (v: number, o: number) => Math.min(odim - 1, Math.max(0, Math.floor(((v + 0.5) / dim - o) / size * odim)));
        for (let y = -2; y < dim + 2; y++) {
            const fy = (y + 0.5) / dim;
            if (fy < oy || fy >= oy + size) continue;
            const v = at(y, oy);
            const inside = y >= 0 && y < dim;
            for (let x = -2; x < dim + 2; x++) {
                if (inside && x === 0) x = dim;
                const fx = (x + 0.5) / dim;
                if (fx < ox || fx >= ox + size) continue;
                this.data[this._idx(x, y)] = other.data[other._idx(at(x, ox), v)];
            }
        }
    }

    private _getByteView(): Uint8Array {
        let byteView = DEMData.byteViewCache.get(this);
        if (byteView?.buffer !== this.data.buffer) {
            byteView = new Uint8Array(this.data.buffer);
            DEMData.byteViewCache.set(this, byteView);
        }
        return byteView;
    }

    private _unpackAtIndex(pixels: Uint8Array, index: number): number {
        return this.unpack(pixels[index], pixels[index + 1], pixels[index + 2]);
    }
}

export function packDEMData(v: number, unpackVector: number[]): {r: number; g: number; b: number} {
    const redFactor = unpackVector[0];
    const greenFactor = unpackVector[1];
    const blueFactor = unpackVector[2];
    const baseShift = unpackVector[3];
    const minScale = Math.min(redFactor, greenFactor, blueFactor);
    const vScaled = Math.round((v + baseShift)/minScale);
    return {
        r: Math.floor(vScaled*minScale/redFactor) % 256,
        g: Math.floor(vScaled*minScale/greenFactor) % 256,
        b: Math.floor(vScaled*minScale/blueFactor) % 256
    };
}

register('DEMData', DEMData);
