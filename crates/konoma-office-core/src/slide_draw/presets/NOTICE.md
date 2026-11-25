# Notice for `presetShapeDefinitions.xml`

`presetShapeDefinitions.xml` in this directory is the definition of the 187 DrawingML preset
shapes (186 distinct names; `upDownArrow` appears twice) published with

> ECMA-376, Office Open XML file formats, Part 1: Fundamentals and Markup Language Reference,
> 5th edition, December 2016 (`OfficeOpenXML-DrawingMLGeometries.zip`, in
> https://ecma-international.org/wp-content/uploads/ECMA-376-1_5th_edition_december_2016.zip)

© Ecma International

The file is included **byte-for-byte unchanged** (the licence below forbids modifying the content).
konoma parses it at run time (`../geom.rs`) to draw preset shapes -- it uses the functionality the
document specifies in a standard-conformant product (clause (iv) below).

**This file is not under konoma's MIT licence.** It is Ecma International's work and stays under
the Ecma copyright licence reproduced here; the licence must travel with every copy of the file.

The licence text below is the default Ecma copyright notice, quoted verbatim from
https://ecma-international.org/policies/by-ipr/ecma-text-copyright-policy/ (Version 3,
approved in December 2025; retrieved 2026-10-09).

---

COPYRIGHT NOTICE

© Ecma International

By obtaining and/or copying this work, you (the licensee) agree that you have read, understood, and will comply with the following terms and conditions.

This document may be copied, published and distributed to others, and certain derivative works of it may be prepared, copied, published, and distributed, in whole or in part, provided that the above copyright notice and this Copyright License and Disclaimer are included on all such copies and derivative works. The only derivative works that are permissible under this Copyright License and Disclaimer are:

(i) works which incorporate all or portion of this document for the purpose of providing commentary or explanation (such as an annotated version of the document),

(ii) works which incorporate all or portion of this document for the purpose of incorporating features that provide accessibility,

(iii) translations of this document into languages other than English and into different formats and

(iv) works by making use of this specification in standard conformant products by implementing (e.g. by copy and paste wholly or partly) the functionality therein.

However, the content of this document itself may not be modified in any way, including by removing the copyright notice or references to Ecma International, except as required to translate it into languages other than English or into a different format.

The official version of an Ecma International document is the English language version on the Ecma International website. In the event of discrepancies between a translated version and the official version, the official version shall govern.

The limited permissions granted above are perpetual and will not be revoked by Ecma International or its successors or assigns.

This document and the information contained herein is provided on an “AS IS” basis and ECMA INTERNATIONAL DISCLAIMS ALL WARRANTIES, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTY THAT THE USE OF THE INFORMATION HEREIN WILL NOT INFRINGE ANY OWNERSHIP RIGHTS OR ANY IMPLIED WARRANTIES OF MERCHANTABILITY OR FITNESS FOR A PARTICULAR PURPOSE.
