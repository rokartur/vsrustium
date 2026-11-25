// Prints the first AXTextArea's AXValue and AXInsertionPointLineNumber of a process as JSON, the attributes VoiceOver
// reads a text editor's line from; with a label, the first whose AXDescription starts with it.
// Used by port/tools/a11y-check: swift port/tools/ax-text.swift <pid> [label]
import ApplicationServices
import Foundation

func attribute(_ element: AXUIElement, _ name: String) -> AnyObject? {
    var value: AnyObject?
    return AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success ? value : nil
}

func textArea(_ element: AXUIElement, depth: Int) -> AXUIElement? {
    if attribute(element, kAXRoleAttribute) as? String == kAXTextAreaRole && (attribute(element, kAXDescriptionAttribute) as? String ?? "").hasPrefix(label) {
        return element
    }
    guard depth < 12, let children = attribute(element, kAXChildrenAttribute) as? [AXUIElement] else {
        return nil
    }
    return children.lazy.compactMap { textArea($0, depth: depth + 1) }.first
}

let pid = pid_t(CommandLine.arguments[1])!
let label = CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : ""
var result: [String: Any] = ["error": "no AXTextArea"]
if let area = textArea(AXUIElementCreateApplication(pid), depth: 0) {
    result = ["value": attribute(area, kAXValueAttribute) as? String ?? NSNull(), "line": attribute(area, kAXInsertionPointLineNumberAttribute) as? Int ?? NSNull()]
}
print(String(data: try JSONSerialization.data(withJSONObject: result), encoding: .utf8)!)
