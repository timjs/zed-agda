# Third-party notices

Parts of `agda-bridge` are ported from [agda2-vscode](https://github.com/willtunnels/agda2-vscode):
the command builders and string quoting (`src/iotcm.rs`), the response types
and prompt handling (`src/protocol.rs`), goal tracking through edits
(`src/goals.rs`, `src/text.rs`) and the location parsing (`src/location.rs`).
`src/abbreviations.json` is copied unchanged from agda2-vscode (commit
`3d715e4`), which generates it from Agda's Emacs input method
(`agda-input.el`, MIT licence); that input method includes the translations of
the TeX input method of GNU Emacs. The licence of agda2-vscode:

```text
MIT License

Copyright (c) 2026 Benjamin Driscoll

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
