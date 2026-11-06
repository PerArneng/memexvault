// Spec 027 corpus helper. Reads JSON lines {"id","voice","text","out"} on stdin, writes 16 kHz-native WAV per line,
// and prints JSON with word markers from AVSpeechSynthesizer: exact sample offsets for each word.
import AVFoundation
import Foundation

final class Job: @unchecked Sendable {
    let lock = NSLock()
    var file: AVAudioFile?
    var words: [[String: Any]] = []
    var frames: Int64 = 0
    var rate: Double = 0
}
_ = AVSpeechSynthesisVoice.speechVoices()
let synth = AVSpeechSynthesizer()
func run(id: String, voice: String, text: String, out: String) {
    let u = AVSpeechUtterance(string: text)
    guard let v = AVSpeechSynthesisVoice(identifier: voice) else { fatalError("voice \(voice)") }
    u.voice = v
    let j = Job()
    let sem = DispatchSemaphore(value: 0)
    let ns = text as NSString
    synth.write(u, toBufferCallback: { buf in
        guard let p = buf as? AVAudioPCMBuffer else { return }
        if p.frameLength == 0 { sem.signal(); return }
        if j.file == nil {
            j.rate = p.format.sampleRate
            j.file = try! AVAudioFile(forWriting: URL(fileURLWithPath: out), settings: p.format.settings, commonFormat: p.format.commonFormat, interleaved: p.format.isInterleaved)
        }
        try! j.file!.write(from: p)
        j.frames += Int64(p.frameLength)
    }, toMarkerCallback: { ms in
        j.lock.lock()
        for m in ms where m.mark == .word {
            j.words.append(["t": ns.substring(with: m.textRange), "byte": m.byteSampleOffset, "loc": m.textRange.location])
        }
        j.lock.unlock()
    })
    _ = sem.wait(timeout: .now() + 120)
    // Markers can arrive after the last audio buffer. The caller rejects a sentence whose markers don't cover its text.
    let expected = text.split(separator: " ").count
    for _ in 0..<40 {
        j.lock.lock(); let n = j.words.count; j.lock.unlock()
        if n >= expected { break }
        Thread.sleep(forTimeInterval: 0.05)
    }
    j.lock.lock(); defer { j.lock.unlock() }
    j.file = nil  // closes the file, which finishes its header
    let o: [String: Any] = ["id": id, "rate": j.rate, "frames": j.frames, "words": j.words]
    print(String(data: try! JSONSerialization.data(withJSONObject: o), encoding: .utf8)!)
    fflush(stdout)
}
DispatchQueue.global().async {
    while let l = readLine() {
        let o = try! JSONSerialization.jsonObject(with: l.data(using: .utf8)!) as! [String: String]
        run(id: o["id"]!, voice: o["voice"]!, text: o["text"]!, out: o["out"]!)
    }
    exit(0)
}
RunLoop.main.run()
