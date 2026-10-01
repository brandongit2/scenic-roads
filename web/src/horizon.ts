// Labels thin toward the horizon in a tilted view (Layers → Map labels → Density, "Toward the
// horizon"). Labels already thin by importance (their tiles' zoom drops with distance), but where
// the ground is foreshortened, near the horizon, a screen's height holds many times the ground it
// does at the centre, and the labels crowd. Here a point label's collision box grows with its
// distance from the camera beyond the view's centre, (distance / centre distance)^k, so labels far
// off need more room than they take up: fewer show there, the most important first (placement
// order). MapLibre has no distance expression for filters, so this patches its collision index
// (on the prototype, once the first placement exists): placeCollisionBox sizes the box by the
// perspective ratio it projects, which is scaled while it runs for a point label. Labels along
// lines and MapLibre's cut-off for labels too far away (perspectiveRatioCutoff) are unchanged.
import type { Map as MLMap } from 'maplibre-gl';

type Projected = { perspectiveRatio: number; signedDistanceFromCamera: number };
type CollisionIndex = {
  transform: { cameraToCenterDistance: number; pitch: number };
  perspectiveRatioCutoff: number;
  placeCollisionBox(...args: unknown[]): unknown;
  projectAndGetPerspectiveRatio(...args: unknown[]): Projected;
  __horizon?: boolean;
};
type Style = { placement?: { collisionIndex?: CollisionIndex; setStale(): void } };

/** The exponent (0: off). */
let k = 0;
/** Within placeCollisionBox for a point label (aligned to the viewport). */
let inPoint = false;

const own = (o: object, key: string) => Object.prototype.hasOwnProperty.call(o, key);

function patch(proto: CollisionIndex) {
  if (own(proto, '__horizon')) return;
  proto.__horizon = true;
  const place = proto.placeCollisionBox;
  proto.placeCollisionBox = function (this: CollisionIndex, ...args: unknown[]) {
    // (collisionBox, overlapMode, textPixelRatio, tileID, unwrappedTileID, pitchWithMap, rotateWithMap, …)
    if (k <= 0 || args[5] || args[6] || this.transform.pitch < 1) return place.apply(this, args);
    inPoint = true;
    try {
      return place.apply(this, args);
    } finally {
      inPoint = false;
    }
  };
  const project = proto.projectAndGetPerspectiveRatio;
  proto.projectAndGetPerspectiveRatio = function (this: CollisionIndex, ...args: unknown[]) {
    const p = project.apply(this, args);
    if (inPoint) {
      const d = p.signedDistanceFromCamera / this.transform.cameraToCenterDistance;
      // Beyond the centre; those past MapLibre's cut-off stay below it (hidden).
      if (d > 1 && p.perspectiveRatio >= this.perspectiveRatioCutoff) p.perspectiveRatio *= d ** k;
    }
    return p;
  };
}

let waiting: (() => void) | null = null;

/** Thinning toward the horizon, 0 (none) … 1 (a label twice as far as the centre needs twice the
 * room); labels placed again. */
export function setHorizonThinning(map: MLMap, value: number) {
  k = value;
  const style = (map as unknown as { style?: Style }).style;
  const ci = style?.placement?.collisionIndex;
  if (!ci) {
    // Before the first placement: once there is one.
    if (!waiting) {
      waiting = () => {
        if (!(map as unknown as { style?: Style }).style?.placement?.collisionIndex) return;
        map.off('render', waiting!);
        waiting = null;
        setHorizonThinning(map, k);
      };
      map.on('render', waiting);
    }
    return;
  }
  patch(Object.getPrototypeOf(ci) as CollisionIndex);
  style!.placement!.setStale();
  map.triggerRepaint();
}
