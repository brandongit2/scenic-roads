// Renders an SVG to a square PNG: render <in.svg> <out.png> <pixels> (tools/status/icon/build.sh).
import AppKit
let a = CommandLine.arguments
guard a.count == 4, let px = Int(a[3]), let img = NSImage(contentsOfFile: a[1]) else {
    FileHandle.standardError.write("usage: render <in.svg> <out.png> <pixels>\n".data(using: .utf8)!)
    exit(2)
}
let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: px, bitsPerSample: 8,
                           samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                           bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
NSGraphicsContext.current!.imageInterpolation = .high
img.draw(in: NSRect(x: 0, y: 0, width: px, height: px))
NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: a[2]))
