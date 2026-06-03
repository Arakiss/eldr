// thermalstate — print the macOS thermal pressure level, one word, no sudo.
//
// The bash prototype shelled out to this tiny helper. Eldr reimplements the same
// reading in pure Rust via the bare Objective-C runtime (see src/ffi/thermal.rs),
// so this file is kept only as the prototype/spec.
//
// Build:  swiftc -O thermalstate.swift -o thermalstate
// Output: nominal | fair | serious | critical | unknown

import Foundation

switch ProcessInfo.processInfo.thermalState {
case .nominal:  print("nominal")
case .fair:     print("fair")
case .serious:  print("serious")
case .critical: print("critical")
@unknown default: print("unknown")
}
