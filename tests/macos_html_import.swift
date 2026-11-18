// Manual macOS importer regression: swift tests/macos_html_import.swift
// Uses only synthetic in-memory data; never accesses the system clipboard.
import AppKit

let bodies = [
    "<html><body><b>功能侧重点不同：中文🍎</b><pre>\n  多行代码</pre></body></html>",
    "<html><head><meta charset=gbk></head><body>功能侧重点不同：中文🍎<pre>\n  多行代码</pre></body></html>"
]
for (index, body) in bodies.enumerated() {
    let bytes = Data([0xef, 0xbb, 0xbf]) + Data(body.utf8)
    let imported = try NSAttributedString(
        data: bytes,
        options: [.documentType: NSAttributedString.DocumentType.html],
        documentAttributes: nil
    )
    // Cocoa can insert HTML block separators; preserve the actual Unicode content.
    precondition(imported.string.contains("功能侧重点不同：中文🍎"))
    precondition(imported.string.contains("  多行代码"))
    print("UTF-8 HTML importer fixture \(index + 1): passed")
}
