// Drives and captures the upstream CAD Studio app on macOS for the gallery.
// Build: swiftc -O macdrive.swift -o macdrive  (the terminal running it needs
// Accessibility and Screen Recording permission).
//
//   macdrive wins                         on-screen app windows: id layer x,y,w,h title
//   macdrive wait <regex> [secs]          until a window title matches; prints its id
//   macdrive cap <out.png> <regex>        that window, no shadow (2x)
//   macdrive capall <out.png> <regex>     that window (its capture already holds attached
//                                         sheets) with the app's overlapping menus and
//                                         popovers drawn on top from their screen rects,
//                                         clipped to the window
//   macdrive find <text>                  AX elements whose title/description/value/
//                                         identifier contains text: role label center
//   macdrive press <text> [n]             AXPress the nth match (default 0); a click
//                                         at its center when AXPress is refused
//   macdrive act <text> <AXAction> [n]    perform an AX action (AXShowMenu, ...)
//   macdrive click <x> <y> [n]            n left clicks (default 1), global points
//   macdrive key <keycode> [cmd|shift|opt]...
//   macdrive type <text>
//   macdrive front                        bring the app forward
import AppKit
import ApplicationServices

let bundleID = "ch.filippofinke.CADStudio"
let args = Array(CommandLine.arguments.dropFirst())
func die(_ s: String) -> Never { FileHandle.standardError.write((s + "\n").data(using: .utf8)!); exit(1) }

func app() -> NSRunningApplication {
  guard let a = NSRunningApplication.runningApplications(withBundleIdentifier: bundleID).first else { die("app not running") }
  return a
}

struct Win { let id: Int; let layer: Int; let rect: CGRect; let title: String }
func windows() -> [Win] {
  guard let pid = NSRunningApplication.runningApplications(withBundleIdentifier: bundleID).first
    .map({ Int($0.processIdentifier) }) else { return [] }
  let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as! [[String: Any]]
  return list.compactMap { w in
    guard (w[kCGWindowOwnerPID as String] as? Int) == pid else { return nil }
    let b = w[kCGWindowBounds as String] as! [String: CGFloat]
    return Win(id: w[kCGWindowNumber as String] as! Int, layer: w[kCGWindowLayer as String] as! Int,
               rect: CGRect(x: b["X"]!, y: b["Y"]!, width: b["Width"]!, height: b["Height"]!),
               title: w[kCGWindowName as String] as? String ?? "")
  }
}
func window(_ pattern: String) -> Win? {
  let re = try! NSRegularExpression(pattern: pattern)
  return windows().filter { $0.layer == 0 && re.firstMatch(in: $0.title, range: NSRange($0.title.startIndex..., in: $0.title)) != nil }
    .max { $0.rect.width * $0.rect.height < $1.rect.width * $1.rect.height }
}
func run(_ path: String, _ a: [String]) {
  let p = Process(); p.executableURL = URL(fileURLWithPath: path); p.arguments = a
  try! p.run(); p.waitUntilExit()
}

func attr(_ e: AXUIElement, _ a: String) -> AnyObject? { var v: AnyObject?; AXUIElementCopyAttributeValue(e, a as CFString, &v); return v }
func label(_ e: AXUIElement) -> String {
  [kAXTitleAttribute, kAXDescriptionAttribute, kAXValueAttribute, kAXIdentifierAttribute]
    .compactMap { attr(e, $0).map { "\($0)" } }.filter { !$0.isEmpty }.joined(separator: " | ")
}
func center(_ e: AXUIElement) -> CGPoint {
  var p = CGPoint.zero, s = CGSize.zero
  if let v = attr(e, kAXPositionAttribute) { AXValueGetValue(v as! AXValue, .cgPoint, &p) }
  if let v = attr(e, kAXSizeAttribute) { AXValueGetValue(v as! AXValue, .cgSize, &s) }
  return CGPoint(x: p.x + s.width / 2, y: p.y + s.height / 2)
}
func matches(_ text: String) -> [AXUIElement] {
  var out: [AXUIElement] = []
  func walk(_ e: AXUIElement, _ d: Int) {
    if label(e).localizedCaseInsensitiveContains(text) { out.append(e) }
    guard d < 40, let kids = attr(e, kAXChildrenAttribute) as? [AXUIElement] else { return }
    kids.forEach { walk($0, d + 1) }
  }
  let root = AXUIElementCreateApplication(app().processIdentifier)
  (attr(root, kAXWindowsAttribute) as? [AXUIElement] ?? []).forEach { walk($0, 0) }
  if let bar = attr(root, kAXMenuBarAttribute) { walk(bar as! AXUIElement, 0) }
  return out
}
// Elements with a label part equal to text rank ahead of substring matches.
func nth(_ text: String, _ n: Int) -> AXUIElement {
  let all = matches(text)
  let exact = { (e: AXUIElement) in label(e).components(separatedBy: " | ").contains(text) }
  let m = all.filter(exact) + all.filter { !exact($0) }
  guard n < m.count else { die("no match \(n) for '\(text)' (\(m.count) found)") }
  return m[n]
}
func post(_ e: CGEvent?) { e?.post(tap: .cghidEventTap); usleep(30_000) }
func click(_ p: CGPoint, _ count: Int = 1) {
  for i in 1...count {
    for t in [CGEventType.leftMouseDown, .leftMouseUp] {
      let e = CGEvent(mouseEventSource: nil, mouseType: t, mouseCursorPosition: p, mouseButton: .left)
      e?.setIntegerValueField(.mouseEventClickState, value: Int64(i)); post(e)
    }
  }
}
// NSRunningApplication.activate cannot take focus from another app on macOS 14+.
func front() {
  guard !app().isActive else { return }
  run("/usr/bin/osascript", ["-e", "tell application id \"\(bundleID)\" to activate"]); usleep(300_000)
}

switch args.first ?? "" {
case "wins":
  for w in windows() { print("\(w.id)\t\(w.layer)\t\(Int(w.rect.minX)),\(Int(w.rect.minY)),\(Int(w.rect.width)),\(Int(w.rect.height))\t\(w.title)") }
case "wait":
  let deadline = Date().addingTimeInterval(Double(args.count > 2 ? args[2] : "10")!)
  while Date() < deadline {
    if let w = window(args[1]) { print(w.id); exit(0) }
    usleep(200_000)
  }
  die("timeout waiting for window '\(args[1])'")
case "cap":
  guard let w = window(args[2]) else { die("no window '\(args[2])'") }
  run("/usr/sbin/screencapture", ["-x", "-o", "-l\(w.id)", args[1]])
case "capall":
  guard let w = window(args[2]) else { die("no window '\(args[2])'") }
  // CGWindowList order is front to back; draw back to front.
  let stack = [w] + windows().filter { $0.layer != 0 && $0.rect.intersects(w.rect) }.reversed()
  let r = w.rect
  let scale = 2.0
  let ctx = CGContext(data: nil, width: Int(r.width * scale), height: Int(r.height * scale), bitsPerComponent: 8,
                      bytesPerRow: 0, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                      bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
  for x in stack {
    let tmp = NSTemporaryDirectory() + "macdrive-\(x.id).png"
    // A menu's window capture returns the whole screen, so overlays come from their rect.
    if x.id == w.id { run("/usr/sbin/screencapture", ["-x", "-o", "-l\(x.id)", tmp]) }
    else { run("/usr/sbin/screencapture", ["-x", "-R\(Int(x.rect.minX)),\(Int(x.rect.minY)),\(Int(x.rect.width)),\(Int(x.rect.height))", tmp]) }
    guard let img = NSImage(contentsOfFile: tmp)?.cgImage(forProposedRect: nil, context: nil, hints: nil) else { continue }
    let f = CGRect(x: (x.rect.minX - r.minX) * scale, y: (r.maxY - x.rect.maxY) * scale,
                   width: x.rect.width * scale, height: x.rect.height * scale)
    ctx.draw(img, in: f)
    try? FileManager.default.removeItem(atPath: tmp)
  }
  try! NSBitmapImageRep(cgImage: ctx.makeImage()!).representation(using: .png, properties: [:])!
    .write(to: URL(fileURLWithPath: args[1]))
case "find":
  for e in matches(args[1]) { let c = center(e); print("\(attr(e, kAXRoleAttribute) ?? "" as AnyObject)\t\(label(e))\t\(Int(c.x)),\(Int(c.y))") }
case "press":
  front()
  let e = nth(args[1], args.count > 2 ? Int(args[2])! : 0)
  if AXUIElementPerformAction(e, kAXPressAction as CFString) != .success { click(center(e)) }
case "act":
  front()
  let e = nth(args[1], args.count > 3 ? Int(args[3])! : 0)
  let r = AXUIElementPerformAction(e, args[2] as CFString); if r != .success { die("\(args[2]) failed: \(r.rawValue)") }
case "click":
  front(); click(CGPoint(x: Double(args[1])!, y: Double(args[2])!), args.count > 3 ? Int(args[3])! : 1)
case "key":
  front()
  var flags: CGEventFlags = []
  for m in args.dropFirst(2) { flags.insert(["cmd": .maskCommand, "shift": .maskShift, "opt": .maskAlternate][m] ?? []) }
  for down in [true, false] {
    let e = CGEvent(keyboardEventSource: nil, virtualKey: CGKeyCode(args[1])!, keyDown: down); e?.flags = flags; post(e)
  }
case "type":
  front()
  for ch in args[1].utf16 {
    for down in [true, false] {
      let e = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: down)
      var c = ch; e?.keyboardSetUnicodeString(stringLength: 1, unicodeString: &c); post(e)
    }
  }
case "front": front()
default: die("usage: see header")
}
