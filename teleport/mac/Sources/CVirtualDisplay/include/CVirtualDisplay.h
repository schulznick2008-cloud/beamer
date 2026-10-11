#ifndef CVIRTUALDISPLAY_H
#define CVIRTUALDISPLAY_H

#include <CoreGraphics/CoreGraphics.h>
#include <stdbool.h>

/// Virtueller Monitor ueber das private CoreGraphics-API CGVirtualDisplay (kein oeffentliches API; Muster wie
/// DeskPad, MIT). Die Schnittstellen werden in CVirtualDisplay.m selbst deklariert, es ist kein fremder Code kopiert.
/// Der Monitor gehoert dem Prozess und verschwindet mit ihm.
typedef struct TPVirtualDisplay TPVirtualDisplay;

/// Legt einen Monitor mit `widthPoints` x `heightPoints` Punkten an (bei `hiDPI` mit doppelter Pixelzahl).
/// NULL bei Fehler (das private API fehlt oder wird abgelehnt).
TPVirtualDisplay *TPVirtualDisplayCreate(unsigned widthPoints, unsigned heightPoints, bool hiDPI,
                                         const char *name, unsigned serial);
CGDirectDisplayID TPVirtualDisplayID(const TPVirtualDisplay *display);
/// Stellt eine neue Groesse ein (Punkte); die Display-ID bleibt.
bool TPVirtualDisplayResize(TPVirtualDisplay *display, unsigned widthPoints, unsigned heightPoints, bool hiDPI);
/// Entfernt den Monitor.
void TPVirtualDisplayDestroy(TPVirtualDisplay *display);

#endif
