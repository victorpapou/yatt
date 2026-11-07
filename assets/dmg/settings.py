# dmgbuild settings (run via scripts/make-dmg.sh; env: APP, NOTE)
import os
app = os.environ["APP"]; note = os.environ["NOTE"]
format = "UDZO"
filesystem = "HFS+"
files = [app, note]
symlinks = {"Applications": "/Applications"}
background = "assets/dmg/background.png"
window_rect = ((200, 120), (560, 360))
default_view = "icon-view"
show_toolbar = False
show_status_bar = False
show_sidebar = False
show_pathbar = False
icon_size = 112
text_size = 13
arrange_by = None
icon_locations = {
    "Yatt.app": (150, 160),
    "Applications": (410, 160),
    "Как открыть.txt": (280, 310),
    # push system dot-folders out of sight for people who show hidden files
    ".background": (900, 900),
    ".fseventsd": (900, 900),
    ".VolumeIcon.icns": (900, 900),
}
