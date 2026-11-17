// Fixed JXA helper. Input and output travel only over the authenticated SSH channel.
// Never insert clipboard text into this command or into exception messages.
ObjC.import("Foundation");
function run() {
    var result = {revision: -1, applied: false, text: null, error: null};
    try {
        var attrs = $.NSFileManager.defaultManager.attributesOfItemAtPathError("/dev/console", null);
        var owner = ObjC.unwrap(attrs.objectForKey($.NSFileOwnerAccountName));
        if (!owner || owner === "root" || owner === "loginwindow" || owner === "_mbsetupuser" || owner !== ObjC.unwrap($.NSUserName())) {
            result.error = "desktop_account";
        } else {
            var data = $.NSFileHandle.fileHandleWithStandardInput.readDataToEndOfFile;
            var input = JSON.parse(ObjC.unwrap($.NSString.alloc.initWithDataEncoding(data, $.NSUTF8StringEncoding)));
            if (!Number.isInteger(input.max_bytes) || input.max_bytes < 1 || input.max_bytes > 1048576) { throw Error(); }
            ObjC.import("AppKit");
            var pb = $.NSPasteboard.generalPasteboard;
            result.revision = Number(pb.changeCount);
            if (input.text !== null) {
                var text = $(input.text);
                if (input.text.length === 0 || input.text.indexOf("\u0000") !== -1 || Number(text.lengthOfBytesUsingEncoding($.NSUTF8StringEncoding)) > input.max_bytes) { throw Error(); }
                // Conditional write: a remote copy after the snapshot must not be silently overwritten.
                // NSPasteboard has no atomic compare-and-set API; keep this check next to the write.
                if (input.since === result.revision) {
                    pb.clearContents;
                    if (!pb.setStringForType(text, $.NSPasteboardTypeString)) { throw Error(); }
                    result.revision = Number(pb.changeCount);
                    result.applied = true;
                }
            } else if (input.since !== null && input.since !== result.revision) {
                var current = pb.stringForType($.NSPasteboardTypeString);
                if (!current.isNil() && Number(current.lengthOfBytesUsingEncoding($.NSUTF8StringEncoding)) <= input.max_bytes) {
                    result.text = ObjC.unwrap(current);
                }
                // A copy racing the read invalidates the snapshot, leaving the next poll to read it.
                if (Number(pb.changeCount) !== result.revision) { result.error = "changed_during_read"; result.text = null; }
            }
        }
    } catch (ignored) { result.error = "helper_failed"; result.text = null; }
    $.NSFileHandle.fileHandleWithStandardOutput.writeData($(JSON.stringify(result)).dataUsingEncoding($.NSUTF8StringEncoding));
}
