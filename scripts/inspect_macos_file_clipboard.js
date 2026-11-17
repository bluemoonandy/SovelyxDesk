// Read only fixed format counts. Never read values, file paths, or clipboard text.
ObjC.import("AppKit");
function summarize(pb) {
    var result = {items: 0, file_urls: 0, finder_nodes: 0, legacy_file_lists: 0,
        file_promises: 0, plain: 0, png: 0, tiff: 0, other: 0, truncated: false};
    var items = pb.pasteboardItems;
    if (items.isNil()) { return result; }
    result.items = Number(items.count);
    result.truncated = result.items > 16;
    for (var i = 0; i < Math.min(result.items, 16); i++) {
        var types = items.objectAtIndex(i).types;
        var count = Number(types.count);
        result.truncated = result.truncated || count > 64;
        for (var j = 0; j < Math.min(count, 64); j++) {
            var type = ObjC.unwrap(types.objectAtIndex(j));
            if (type === "public.file-url") { result.file_urls++; }
            else if (type === "com.apple.finder.node") { result.finder_nodes++; }
            else if (type === "NSFilenamesPboardType" || type === "com.apple.pasteboard.legacy-file-list") { result.legacy_file_lists++; }
            else if (type.indexOf("com.apple.pasteboard.promised-file") === 0 || type === "com.apple.pasteboard.NSFilePromiseID") { result.file_promises++; }
            else if (type === "public.utf8-plain-text" || type === "public.utf16-plain-text") { result.plain++; }
            else if (type === "public.png") { result.png++; }
            else if (type === "public.tiff") { result.tiff++; }
            else { result.other++; }
        }
    }
    return result;
}
function run(argv) {
    if (argv.length === 1 && argv[0] === "--self-test") {
        var pb = $.NSPasteboard.pasteboardWithUniqueName;
        try {
            pb.clearContents;
            pb.setStringForType("fixture", $.NSPasteboardTypeString);
            var text = summarize(pb);
            if (text.items !== 1 || text.plain < 1 || text.file_urls !== 0) { throw Error("count regression"); }
            pb.clearContents;
            pb.setStringForType("file:///tmp/maccanvas-format-fixture.txt", $.NSPasteboardTypeFileURL);
            var files = summarize(pb);
            if (files.items !== 1 || files.file_urls !== 1) { throw Error("file count regression"); }
            return "PASS: private-pasteboard format counts; no system clipboard access";
        } finally { pb.releaseGlobally; }
    }
    if (argv.length !== 0) { throw Error("Usage: osascript -l JavaScript inspect_macos_file_clipboard.js [--self-test]"); }
    return JSON.stringify(summarize($.NSPasteboard.generalPasteboard));
}
