## KaTeX code and data in the RaTeX crates (MIT)

The math renderer crates konoma links contain code and data ported or generated from
KaTeX (<https://github.com/KaTeX/KaTeX>): for example the font metrics in `ratex-font`
(generated from `fontMetricsData.js`) and the Unicode script table in `ratex-types`
(`unicodeScripts.ts`). The crates do not ship KaTeX's notice. `LICENSE` of KaTeX at commit
`9b2280dee4acb4da7d866dbaccad120b00949c27`:

```text
The MIT License (MIT)

Copyright (c) 2013-2020 Khan Academy and other contributors

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

## mhchem for KaTeX, in `ratex-parser`

`ratex-parser` is described by its own source as a port of KaTeX mhchem 3.3.0, and embeds
data generated from it (`src/mhchem/data/*.json`). The header comment of
`contrib/mhchem/mhchem.js` at the same KaTeX commit follows. The full Apache License 2.0
text it refers to appears in the crate license list above.

```text
/*************************************************************
 *
 *  KaTeX mhchem.js
 *
 *  This file implements a KaTeX version of mhchem version 3.3.0.
 *  It is adapted from MathJax/extensions/TeX/mhchem.js
 *  It differs from the MathJax version as follows:
 *    1. The interface is changed so that it can be called from KaTeX, not MathJax.
 *    2. \rlap and \llap are replaced with \mathrlap and \mathllap.
 *    3. Four lines of code are edited in order to use \raisebox instead of \raise.
 *    4. The reaction arrow code is simplified. All reaction arrows are rendered
 *       using KaTeX extensible arrows instead of building non-extensible arrows.
 *    5. \tripledash vertical alignment is slightly adjusted.
 *
 *    This code, as other KaTeX code, is released under the MIT license.
 * 
 * /*************************************************************
 *
 *  MathJax/extensions/TeX/mhchem.js
 *
 *  Implements the \ce command for handling chemical formulas
 *  from the mhchem LaTeX package.
 *
 *  ---------------------------------------------------------------------
 *
 *  Copyright (c) 2011-2015 The MathJax Consortium
 *  Copyright (c) 2015-2018 Martin Hensel
 *
 *  Licensed under the Apache License, Version 2.0 (the "License");
 *  you may not use this file except in compliance with the License.
 *  You may obtain a copy of the License at
 *
 *      http://www.apache.org/licenses/LICENSE-2.0
 *
 *  Unless required by applicable law or agreed to in writing, software
 *  distributed under the License is distributed on an "AS IS" BASIS,
 *  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 *  See the License for the specific language governing permissions and
 *  limitations under the License.
 */
```
