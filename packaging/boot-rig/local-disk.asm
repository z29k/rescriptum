; The marker a machine prints when it falls through the menu to its own disk.
;
; **This is the whole of the unclaimed-machine assertion**, and it is deterministic
; rather than a screenshot: QEMU's serial console goes to a log, and the rig greps it.
; A machine that PXE-boots by accident, and that nothing claims, must end up here —
; never installing anything, never sitting at a menu waiting for a human who is not
; coming. The worst case of being wrong about which machines reach the boot server is
; a few seconds added to a boot, and this is what proves it.
;
; Assembled by the client image's nasm into a 512-byte MBR. Nothing else is on the disk:
; if the firmware reaches this, it reached the disk.

[BITS 16]
[ORG 0x7C00]

start:
    cli
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7C00
    sti

    ; COM1 at 115200 8N1. QEMU would emit the bytes without this, but relying on that
    ; is relying on an implementation detail of one emulator, and the rig is supposed
    ; to resemble a machine.
    mov dx, 0x03FB          ; line control
    mov al, 0x80            ; divisor latch access
    out dx, al
    mov dx, 0x03F8          ; divisor low
    mov al, 0x01            ; 115200
    out dx, al
    mov dx, 0x03F9          ; divisor high
    xor al, al
    out dx, al
    mov dx, 0x03FB
    mov al, 0x03            ; 8 bits, no parity, one stop; latch off
    out dx, al

    mov si, message
.next:
    lodsb
    test al, al
    jz .done
.wait:
    mov dx, 0x03FD          ; line status
    push ax
    in al, dx
    test al, 0x20           ; transmit holding register empty
    pop ax
    jz .wait
    mov dx, 0x03F8
    out dx, al
    jmp .next

.done:
    cli
.halt:
    hlt
    jmp .halt

message: db 13, 10, "RESCRIPTUM-RIG-LOCAL-DISK-REACHED", 13, 10, 0

times 510-($-$$) db 0
dw 0xAA55
