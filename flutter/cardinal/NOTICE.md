# Third-party notices

The Cardinal Maps app is licensed under the GPL-3.0 (see `LICENSE.md` at the top of the
repository). It includes the following works by others.

## Mapbox directions-icons

The maneuver icons in navigation's banner, in `tool/icons/maneuvers/` and built into
`assets/fonts/CardinalIcons.ttf` by `tool/icon_font.py`, are Mapbox's directions-icons:

- Source: https://github.com/mapbox/directions-icons, `src/svg/`, at commit
  `9016ba92176cf1a8207cc1c4005b951fe49d59cb`.
- License: CC0 1.0 Universal (public domain dedication); the full text is in
  `tool/icons/maneuvers/LICENSE.md`.
- Changes: only the icons a maneuver can map to are kept, and two are renamed as
  Ferrostar's iOS copies are, `uturn` to `turn_uturn` and `notificaiton_sharp_right` to
  `notification_sharp_right`.

## Ferrostar

Navigation follows Ferrostar's default user interface, and rounds distances to the next
maneuver by the rules of Ferrostar's Android `LocalizedDistanceFormatter`
(`android/ui-formatters/.../DistanceFormatter.kt`, at commit
`6a03904eda084cc2a74840ee0b94d1003bb5f9e0`), under this license:

```
Copyright (c) 2023, Stadia Maps, Inc.

All rights reserved.

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

    * Redistributions of source code must retain the above copyright notice,
      this list of conditions and the following disclaimer.
    * Redistributions in binary form must reproduce the above copyright notice,
      this list of conditions and the following disclaimer in the documentation
      and/or other materials provided with the distribution.
    * Neither the name of the copyright holder nor the names of its contributors
      may be used to endorse or promote products derived from this software
      without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR
CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL,
EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO,
PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```
