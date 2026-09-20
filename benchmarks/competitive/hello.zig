const std = @import("std");

pub fn main() !void {
    const io = std.Io.Threaded.global_single_threaded.io();
    var buffer: [64]u8 = undefined;
    var writer = std.Io.File.stdout().writer(io, &buffer);
    try writer.interface.writeAll("hello from zig\n");
    try writer.interface.flush();
}
