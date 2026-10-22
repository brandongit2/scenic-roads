import {describe, test, expect, vi} from 'vitest';
import {DEMData} from './dem_data.ts';
import {RGBAImage} from '../util/image.ts';
import {serialize, deserialize} from '../util/web_worker_transfer.ts';

function createMockImage(height, width) {
    // RGBAImage passed to constructor has uniform 2px padding on all sides.
    height += 4;
    width += 4;
    const pixels = new Uint8Array(height * width * 4);
    for (let i = 0; i < pixels.length; i++) {
        pixels[i] = (i + 1) % 4 === 0 ? 1 : Math.floor(Math.random() * 256);
    }
    return new RGBAImage({height, width}, pixels);
}

function createMockClampImage(height, width) {
    const pixels = new Uint8ClampedArray(height * width * 4);
    for (let i = 0; i < pixels.length; i++) {
        pixels[i] = (i + 1) % 4 === 0 ? 1 : Math.floor(Math.random() * 256);
    }
    return new RGBAImage({height, width}, pixels);
}

describe('DEMData', () => {
    describe('constructor', () => {
        test('Uint8Array', () => {
            const imageData0 = createMockImage(4, 4);
            const dem = new DEMData('0', imageData0, 'mapbox');
            expect(dem.uid).toBe('0');
            expect(dem.dim).toBe(4);
            expect(dem.stride).toBe(8);
        });

        test('Uint8ClampedArray', () => {
            const imageData0 = createMockClampImage(6, 6);
            const dem = new DEMData('0', imageData0, 'mapbox');
            expect(dem).not.toBeNull();
            expect(dem['uid']).toBe('0');
            expect(dem['dim']).toBe(2);
            expect(dem['stride']).toBe(6);
        });

        test('otherEncoding', () => {
            const spyOnWarnConsole = vi.spyOn(console, 'warn').mockImplementation(() => {});

            const imageData0 = createMockImage(4, 4);
            new DEMData('0', imageData0, 'otherEncoding' as any);

            expect(spyOnWarnConsole).toHaveBeenCalledTimes(1);
            expect(spyOnWarnConsole.mock.calls).toEqual([['\"otherEncoding\" is not a valid encoding type. Valid types include \"mapbox\", \"terrarium\" and \"custom\".']]);
        });
    });
});

function expectDEMBorderRegion(dem: DEMData) {
    return () => {
        let nonempty = true;
        for (let x = -1; x < 5; x++) {
            for (let y = -1; y < 5; y++) {
                if (dem.get(x, y) === -65536) {
                    nonempty = false;
                    break;
                }
            }
        }
        expect(nonempty).toBeTruthy();

        let verticalBorderMatch = true;
        for (const x of [-1, 4]) {
            for (let y = 0; y < 4; y++) {
                if (dem.get(x, y) !== dem.get(x < 0 ? x + 1 : x - 1, y)) {
                    verticalBorderMatch = false;
                    break;
                }
            }
        }
        expect(verticalBorderMatch).toBeTruthy();

        // horizontal borders empty
        let horizontalBorderMatch = true;
        for (const y of [-1, 4]) {
            for (let x = 0; x < 4; x++) {
                if (dem.get(x, y) !== dem.get(x, y < 0 ? y + 1 : y - 1)) {
                    horizontalBorderMatch = false;
                    break;
                }
            }
        }
        expect(horizontalBorderMatch).toBeTruthy();

        expect(dem.get(-1, 4) === dem.get(0, 3)).toBeTruthy();
        expect(dem.get(4, 4) === dem.get(3, 3)).toBeTruthy();
        expect(dem.get(-1, -1) === dem.get(0, 0)).toBeTruthy();
        expect(dem.get(4, -1) === dem.get(3, 0)).toBeTruthy();
    };
}

function expectDEMBackfill(dem0: DEMData, dem1: DEMData) {
    return  () => {
        dem0.backfillFrom(dem1, -1, 0, 1);
        for (let y = 0; y < 4; y++) {
            // dx = -1, dy = 0, so the left edge of dem1 should equal the right edge of dem0
            expect(dem0.get(-1, y) === dem1.get(3, y)).toBeTruthy();
        }

        dem0.backfillFrom(dem1, 0, -1, 1);
        for (let x = 0; x < 4; x++) {
            expect(dem0.get(x, -1) === dem1.get(x, 3)).toBeTruthy();
        }

        dem0.backfillFrom(dem1, 1, 0, 1);
        for (let y = 0; y < 4; y++) {
            expect(dem0.get(4, y) === dem1.get(0, y)).toBeTruthy();
        }

        dem0.backfillFrom(dem1, 0, 1, 1);
        for (let x = 0; x < 4; x++) {
            expect(dem0.get(x, 4) === dem1.get(x, 0)).toBeTruthy();
        }

        dem0.backfillFrom(dem1, -1, 1, 1);
        expect(dem0.get(-1, 4) === dem1.get(3, 0)).toBeTruthy();

        dem0.backfillFrom(dem1, 1, 1, 1);
        expect(dem0.get(4, 4) === dem1.get(0, 0)).toBeTruthy();

        dem0.backfillFrom(dem1, -1, -1, 1);
        expect(dem0.get(-1, -1) === dem1.get(3, 3)).toBeTruthy();

        dem0.backfillFrom(dem1, 1, -1, 1);
        expect(dem0.get(4, -1) === dem1.get(0, 3)).toBeTruthy();
    };
}

describe('DEMData.backfillFrom with encoding', () => {
    describe('mapbox encoding', () => {
        const dem0 = new DEMData('0', createMockImage(4, 4), 'mapbox');
        const dem1 = new DEMData('1', createMockImage(4, 4), 'mapbox');

        test('border region is initially populated with neighboring data', expectDEMBorderRegion(dem0));
        test('backfillFrom correctly populates borders with neighboring data', expectDEMBackfill(dem0, dem1));
    });

    describe('terrarium encoding', () => {
        const dem0 = new DEMData('0', createMockImage(4, 4), 'terrarium');
        const dem1 = new DEMData('1', createMockImage(4, 4), 'terrarium');

        test('border region is initially populated with neighboring data', expectDEMBorderRegion(dem0));
        test('backfillFrom correctly populates borders with neighboring data', expectDEMBackfill(dem0, dem1));
    });
});

describe('DEMData.backfillFrom across zooms', () => {
    // A terrarium tile `dim` pixels across whose height is `f` of each pixel centre in world units
    // (the tile covering [x0, x0 + size) × [y0, y0 + size)).
    function plane(dim: number, x0: number, y0: number, size: number, f: (x: number, y: number) => number) {
        const stride = dim + 4;
        const pixels = new Uint8Array(stride * stride * 4);
        for (let y = 0; y < dim; y++) {
            for (let x = 0; x < dim; x++) {
                const v = f(x0 + (x + 0.5) / dim * size, y0 + (y + 0.5) / dim * size) + 32768;
                const i = ((y + 2) * stride + x + 2) * 4;
                pixels[i] = Math.floor(v / 256);
                pixels[i + 1] = Math.floor(v) % 256;
                pixels[i + 2] = Math.round((v % 1) * 256);
                pixels[i + 3] = 255;
            }
        }
        return new DEMData('0', new RGBAImage({width: stride, height: stride}, pixels), 'terrarium');
    }
    const f = (x: number, y: number) => 100 * x + 50 * y;

    test('a coarser neighbour, bilinearly', () => {
        const tile = plane(4, 0, 0, 1, f);
        // east, a zoom coarser: [1, 3) × [0, 2)
        tile.backfillFrom(plane(4, 1, 0, 2, f), 1, 0, 2);
        for (let y = 0; y < 4; y++) {
            // (its first pixel centres are at 1.25 and 0.25: the border's first column, at 1.125,
            // and the first row, at 0.125, clamp to them)
            const at = Math.max(0.25, (y + 0.5) / 4);
            expect(tile.get(4, y)).toBeCloseTo(f(1.25, at), 1);
            expect(tile.get(5, y)).toBeCloseTo(f(1.375, at), 1);
        }
        // the north border isn't the neighbour's: as it was
        expect(tile.get(1, -1)).toBe(tile.get(1, 0));
    });

    test('a finer neighbour, exactly where its pixels meet ours', () => {
        const tile = plane(4, 0, 0, 1, f);
        // south-west, a zoom finer: [-0.5, 0) × [1, 1.5)
        tile.backfillFrom(plane(4, -0.5, 1, 0.5, f), -0.5, 1, 0.5);
        expect(tile.get(-1, 4)).toBeCloseTo(f(-0.125, 1.125), 1);
        expect(tile.get(-2, 5)).toBeCloseTo(f(-0.375, 1.375), 1);
        // only its corner of the border
        expect(tile.get(-1, 3)).toBe(tile.get(0, 3));
    });
});

describe('DEMData.sampleBilinear', () => {
    test('interpolates four neighboring pixels', () => {
        const elevations = [
            0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0,
            0, 0, 10, 20, 0, 0,
            0, 0, 30, 40, 0, 0,
            0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0
        ];
        const pixels = new Uint8Array(elevations.flatMap(elevation => [elevation, 0, 0, 0]));
        const dem = new DEMData('sample', new RGBAImage({height: 6, width: 6}, pixels), 'custom', 1.0, 0.0, 0.0, 0.0);

        expect(dem.sampleBilinear(0, 0)).toBe(10);
        expect(dem.sampleBilinear(0.5, 0.5)).toBe(25);
        expect(dem.sampleBilinear(-0.5, 0.5)).toBe(20);
        expect(dem.sampleBilinear(1, 1)).toBe(40);
    });

    test('throws when the bilinear footprint is outside the padded DEM', () => {
        const dem = new DEMData('sample', createMockImage(2, 2), 'custom', 1.0, 1.0, 1.0, 0.0);

        expect(() => dem.sampleBilinear(2, 0)).toThrow(RangeError);
    });
});

function expectSerialization(dem0: DEMData, redFactor: number, greenFactor: number, blueFactor: number, baseShift: number) {
    return () => {
        const serialized = serialize(dem0);

        // calculate min/max values
        let min = Number.MAX_SAFE_INTEGER;
        let max = Number.MIN_SAFE_INTEGER;
        for (let x = 0; x < 4; x++) {
            for (let y = 0; y < 4; y++) {
                const ele = dem0.get(x, y);
                if (ele > max) max = ele;
                if (ele < min) min = ele;
            }
        }

        expect(serialized).toEqual({
            $name: 'DEMData',
            uid: '0',
            dim: 4,
            stride: 8,
            data: dem0.data,
            redFactor,
            greenFactor,
            blueFactor,
            baseShift,
            gridMin: dem0.gridMin,
            gridMax: dem0.gridMax,
            max,
            min,
        });

        const transferrables = [];
        serialize(dem0, transferrables);
        expect(new Uint32Array(transferrables[0])).toEqual(dem0.data);
    };
}

function expectDeserialization(dem0: DEMData) {
    return () => {
        const serialized = serialize(dem0);

        const deserialized = deserialize(serialized);
        expect(deserialized).toEqual(dem0);
    };
}

describe('DEMData is correctly serialized and deserialized', () => {
    const mapboxDEM = new DEMData('0', createMockImage(4, 4), 'mapbox');
    const terrariumDEM = new DEMData('0', createMockImage(4, 4), 'terrarium');
    const customDEM = new DEMData('0', createMockImage(4, 4), 'custom', 1.0, 2.0, 3.0, 4.0);
    test('serialized - mapbox', expectSerialization(mapboxDEM, 6553.6, 25.6, 0.1, 10000));
    test('serialized - terrarium', expectSerialization(terrariumDEM, 256.0, 1.0, 1.0 / 256.0, 32768.0));
    test('serialized - custom', expectSerialization(customDEM, 1.0, 2.0, 3.0, 4.0));

    test('deserialized - mapbox', expectDeserialization(mapboxDEM));
    test('deserialized - terrarium', expectDeserialization(terrariumDEM));
    test('deserialized - custom', expectDeserialization(customDEM));
});

describe('UnpackVector is correctly returned', () => {
    test('terrarium, mapbox and custom', () => {
        const mapboxDEM = new DEMData('0', createMockImage(4, 4), 'mapbox');
        const terrariumDEM = new DEMData('0', createMockImage(4, 4), 'terrarium');
        const customDEM = new DEMData('0', createMockImage(4, 4), 'custom', 1.0, 2.0, 3.0, 4.0);

        expect(terrariumDEM.getUnpackVector()).toEqual([256.0, 1.0, 1.0 / 256.0, 32768.0]);
        expect(mapboxDEM.getUnpackVector()).toEqual([6553.6, 25.6, 0.1, 10000.0]);
        expect(customDEM.getUnpackVector()).toEqual([1.0, 2.0, 3.0, 4.0]);
    });
});

function expectGetPixels(dem: DEMData, imageData: RGBAImage) {
    return () => {
        expect(dem.getPixels()).toEqual(imageData);
    };
}

describe('DEMData.getImage', () => {
    const imageData = createMockImage(4, 4);
    const mapboxDEM = new DEMData('0', imageData, 'terrarium');
    const terrariumDEM = new DEMData('0', imageData, 'terrarium');
    const customDEM = new DEMData('0', imageData, 'terrarium');

    test('Image is correctly returned - mapbox', expectGetPixels(mapboxDEM, imageData));
    test('Image is correctly returned - terrarium', expectGetPixels(terrariumDEM, imageData));
    test('Image is correctly returned - custom', expectGetPixels(customDEM, imageData));
});

describe('DEMData pack and unpack', () => {
    const imageData = createMockImage(4, 4);
    test('mapbox', () => {
        const dem = new DEMData('0', imageData, 'mapbox');
        expect(dem.unpack(123, 177, 215)).toEqual(800645.5);
        expect(dem.pack(800645.5)).toEqual({r: 123, g: 177, b: 215});

        expect(dem.unpack(0, 0, 0)).toBe(-10000);
        expect(dem.pack(-10000)).toEqual({r: 0, g: 0, b: 0});

        expect(dem.unpack(1, 1, 1)).toBeCloseTo(-3420.7);
        expect(dem.pack(-3420.7)).toEqual({r: 1, g: 1, b: 1});

        expect(dem.unpack(255, 255, 255)).toEqual(1667721.5);
        expect(dem.pack(1667721.5)).toEqual({r: 255, g: 255, b: 255});

        expect(dem.unpack(255, 0, 255)).toEqual(1661193.5);
        expect(dem.pack(1661193.5)).toEqual({r: 255, g: 0, b: 255});
    });
    
    test('terrarium', () => {
        const dem = new DEMData('0', imageData, 'terrarium');
        expect(dem.unpack(123, 177, 215)).toBe(-1102.16015625);
        expect(dem.pack(-1102.16015625)).toEqual({r: 123, g: 177, b: 215});

        expect(dem.unpack(0, 0, 0)).toBe(-32768);
        expect(dem.pack(-32768)).toEqual({r: 0, g: 0, b: 0});

        expect(dem.unpack(1, 1, 1)).toBe(-32510.99609375);
        expect(dem.pack(-32510.99609375)).toEqual({r: 1, g: 1, b: 1});

        expect(dem.unpack(255, 255, 255)).toEqual(32767.99609375);
        expect(dem.pack(32767.99609375)).toEqual({r: 255, g: 255, b: 255});

        expect(dem.unpack(255, 0, 255)).toEqual(32512.99609375);
        expect(dem.pack(32512.99609375)).toEqual({r: 255, g: 0, b: 255});
    });
    
    test('custom', () => {
        const dem = new DEMData('0', imageData, 'custom', 0.25, 64, 16384, 7000.0);
        expect(dem.unpack(123, 177, 215)).toEqual(3526918.75);
        expect(dem.pack(3526918.75)).toEqual({r: 123, g: 177, b: 215});

        expect(dem.unpack(0, 0, 0)).toBe(-7000);
        expect(dem.pack(-7000)).toEqual({r: 0, g: 0, b: 0});

        expect(dem.unpack(1, 1, 1)).toEqual(9448.25);
        expect(dem.pack(9448.25)).toEqual({r: 1, g: 1, b: 1});

        expect(dem.unpack(255, 255, 255)).toEqual(4187303.75);
        expect(dem.pack(4187303.75)).toEqual({r: 255, g: 255, b: 255});

        expect(dem.unpack(255, 0, 255)).toEqual(4170983.75);
        expect(dem.pack(4170983.75)).toEqual({r: 255, g: 0, b: 255});
    });
});

describe('DEMData.getMinMaxInRegion (Scenic Roads)', () => {
    // A terrarium tile whose elevation at pixel (x, y) is 3x + 2y + 100 metres, with one spike.
    const dim = 64;
    const elevationAt = (x: number, y: number) => (x === 40 && y === 9) ? 5000 : 3 * x + 2 * y + 100;
    const size = dim + 4;
    const pixels = new Uint8Array(size * size * 4);
    for (let y = 0; y < size; y++) {
        for (let x = 0; x < size; x++) {
            const e = elevationAt(Math.min(dim - 1, Math.max(0, x - 2)), Math.min(dim - 1, Math.max(0, y - 2))) + 32768;
            const i = (y * size + x) * 4;
            pixels[i] = Math.floor(e / 256);
            pixels[i + 1] = Math.floor(e) % 256;
            pixels[i + 2] = Math.round((e % 1) * 256);
            pixels[i + 3] = 255;
        }
    }
    const dem = new DEMData('0', new RGBAImage({width: size, height: size}, pixels), 'terrarium');

    const bruteForce = (x0: number, y0: number, x1: number, y1: number) => {
        let min = Infinity, max = -Infinity;
        for (let y = 0; y < dim; y++) {
            for (let x = 0; x < dim; x++) {
                if ((x + 1) / dim < x0 || x / dim > x1 || (y + 1) / dim < y0 || y / dim > y1) continue;
                min = Math.min(min, elevationAt(x, y));
                max = Math.max(max, elevationAt(x, y));
            }
        }
        return {min, max};
    };

    test('the whole tile is the tile\'s range', () => {
        expect(dem.getMinMaxInRegion(0, 0, 1, 1)).toEqual({min: dem.min, max: dem.max});
        expect(dem.max).toBe(5000);
    });

    test('a region\'s range holds its pixels\', a small one\'s tighter than the tile\'s', () => {
        for (const [x0, y0, s] of [[0, 0, 0.5], [0.5, 0.5, 0.5], [0.25, 0.75, 0.25], [0.125, 0.5, 0.125], [0.875, 0.875, 0.0625], [0.5, 0, 0.03125]]) {
            const got = dem.getMinMaxInRegion(x0, y0, x0 + s, y0 + s);
            const want = bruteForce(x0, y0, x0 + s, y0 + s);
            expect(got.min).toBeLessThanOrEqual(want.min);
            expect(got.max).toBeGreaterThanOrEqual(want.max);
            if (s <= 0.125) expect(got.max - got.min).toBeLessThan(dem.max - dem.min);
        }
    });

    test('the spike counts only in the regions near it', () => {
        expect(dem.getMinMaxInRegion(0.5, 0, 0.75, 0.25).max).toBe(5000);
        expect(dem.getMinMaxInRegion(0, 0.5, 0.5, 1).max).toBeLessThan(5000);
    });

    test('a tile smaller than the grid has every cell filled', () => {
        const small = new DEMData('0', createMockImage(4, 4), 'terrarium');
        expect(small.gridMin.every((v) => Number.isFinite(v))).toBe(true);
        expect(small.gridMax.every((v) => Number.isFinite(v))).toBe(true);
        const {min, max} = small.getMinMaxInRegion(0.6, 0.6, 0.62, 0.62);
        expect(min).toBeLessThanOrEqual(small.get(2, 2));
        expect(max).toBeGreaterThanOrEqual(small.get(2, 2));
        expect(Number.isFinite(min) && Number.isFinite(max)).toBe(true);
    });

    test('the grids survive the transfer from the worker', () => {
        const copy = deserialize(serialize(dem)) as DEMData;
        expect(copy.getMinMaxInRegion(0, 0.5, 0.5, 1)).toEqual(dem.getMinMaxInRegion(0, 0.5, 0.5, 1));
    });
});
