import AppKit

let arguments = CommandLine.arguments
guard arguments.count == 2 else {
    fputs("usage: make-macos-icon.swift OUTPUT_ICONSET\n", stderr)
    exit(2)
}

let output = URL(fileURLWithPath: arguments[1], isDirectory: true)
try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)

let variants: [(points: Int, scale: Int, name: String)] = [
    (16, 1, "icon_16x16.png"),
    (16, 2, "icon_16x16@2x.png"),
    (32, 1, "icon_32x32.png"),
    (32, 2, "icon_32x32@2x.png"),
    (128, 1, "icon_128x128.png"),
    (128, 2, "icon_128x128@2x.png"),
    (256, 1, "icon_256x256.png"),
    (256, 2, "icon_256x256@2x.png"),
    (512, 1, "icon_512x512.png"),
    (512, 2, "icon_512x512@2x.png")
]

func diamond(centerX: CGFloat, centerY: CGFloat, width: CGFloat, height: CGFloat) -> NSBezierPath {
    let path = NSBezierPath()
    path.move(to: NSPoint(x: centerX, y: centerY + height / 2))
    path.line(to: NSPoint(x: centerX + width / 2, y: centerY))
    path.line(to: NSPoint(x: centerX, y: centerY - height / 2))
    path.line(to: NSPoint(x: centerX - width / 2, y: centerY))
    path.close()
    return path
}

for variant in variants {
    let pixels = variant.points * variant.scale
    let size = NSSize(width: pixels, height: pixels)
    let image = NSImage(size: size)
    image.lockFocus()

    NSGraphicsContext.current?.imageInterpolation = .high
    let inset = CGFloat(pixels) * 0.055
    let background = NSBezierPath(
        roundedRect: NSRect(x: inset, y: inset, width: CGFloat(pixels) - inset * 2, height: CGFloat(pixels) - inset * 2),
        xRadius: CGFloat(pixels) * 0.225,
        yRadius: CGFloat(pixels) * 0.225
    )
    NSColor(calibratedWhite: 0.965, alpha: 1).setFill()
    background.fill()

    let center = CGFloat(pixels) / 2
    let width = CGFloat(pixels) * 0.47
    let height = CGFloat(pixels) * 0.225
    let gap = CGFloat(pixels) * 0.105

    NSColor(calibratedWhite: 0.08, alpha: 0.98).setFill()
    diamond(centerX: center, centerY: center + gap, width: width, height: height).fill()
    NSColor(calibratedWhite: 0.08, alpha: 0.70).setFill()
    diamond(centerX: center, centerY: center, width: width, height: height).fill()
    NSColor(calibratedWhite: 0.08, alpha: 0.42).setFill()
    diamond(centerX: center, centerY: center - gap, width: width, height: height).fill()

    image.unlockFocus()

    guard let tiff = image.tiffRepresentation,
          let bitmap = NSBitmapImageRep(data: tiff),
          let png = bitmap.representation(using: .png, properties: [:]) else {
        fputs("failed to render \(variant.name)\n", stderr)
        exit(1)
    }
    try png.write(to: output.appendingPathComponent(variant.name))
}
