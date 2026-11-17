// Independent pasteboard only: never reads or modifies the user clipboard.
// Validate both AppKit and the Carbon API used by Apple clipboard components.
import AppKit
import Carbon
let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
defer { try? FileManager.default.removeItem(at: directory) }
let sourceURL = directory.appendingPathComponent("fixture 中文.txt")
try Data("fixture".utf8).write(to: sourceURL)
for url in [sourceURL, directory.appendingPathComponent("missing.txt")] {
print("Source exists: \(FileManager.default.fileExists(atPath: url.path))")
for useCarbon in [false, true] {
 let pb = NSPasteboard.withUniqueName()
 defer { pb.releaseGlobally() }
 if useCarbon {
  var ref: Pasteboard?
  let status = PasteboardCreate(pb.name.rawValue as CFString, &ref)
  precondition(status == noErr)
  let cleared = PasteboardClear(ref!)
  precondition(cleared == noErr)
  print("Carbon clear status: \(cleared)")
  let data = Data(url.absoluteString.utf8)
  let put = PasteboardPutItemFlavor(ref!, UnsafeMutableRawPointer(bitPattern: 1)!, "public.file-url" as CFString, data as CFData, PasteboardFlavorFlags(rawValue: 0))
  precondition(put == noErr)
  print("Carbon raw URL write status: \(put)")
 } else {
  let item = NSPasteboardItem()
  item.setData(Data(url.absoluteString.utf8), forType: .fileURL)
  precondition(pb.writeObjects([item]))
 }
 let read = pb.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
 precondition(read == [url])
 print("\(useCarbon ? "Carbon" : "AppKit") file URL readable: \(read == [url]), items: \(pb.pasteboardItems?.count ?? 0)")
}

}
