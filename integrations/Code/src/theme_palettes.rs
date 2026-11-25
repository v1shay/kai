// Palette values adapted from Monocode, Copyright (c) 2026 Nick, MIT.
// Source commit 74b8139c41e7235e7535bdc8ba60e63705351d3c; see LICENSES/Monocode-Theme-MIT.txt.
// No web renderer or JavaScript runtime is used.
const PALETTES: [[Palette; 2]; 11] = [
    [
        // Default
        Palette {
            surface: 0x0d0d0d,
            panel_solid: 0x141414,
            content: 0xebebeb,
            accent: 0x4da3f5,
            link: 0x7dd3fc,
            heading: 0xf9a8c9,
            syntax: [
                0x569cd6, 0xdcdcaa, 0xce9178, 0x4ec9b0, 0x569cd6, 0xb5cea8, 0x6a9955, 0x9cdcfe,
                0xdcdcaa, 0xf44747,
            ],
        },
        Palette {
            surface: 0xf5f5f5,
            panel_solid: 0xffffff,
            content: 0x2e2e2e,
            accent: 0x4da3f5,
            link: 0x0866c6,
            heading: 0xbe185d,
            syntax: [
                0x0000ff, 0x795e26, 0xa31515, 0x267f99, 0x800000, 0x098658, 0x008000, 0x001080,
                0x795e26, 0xcd3131,
            ],
        },
    ],
    [
        // Harbor
        Palette {
            surface: 0x0b1018,
            panel_solid: 0x121a26,
            content: 0xdbe4f0,
            accent: 0x5b9cf2,
            link: 0x8cc4ff,
            heading: 0xa3b6ff,
            syntax: [
                0xbb9af7, 0x7aa2f7, 0x9ece6a, 0x2ac3de, 0xf7768e, 0xff9e64, 0x6b7599, 0x73daca,
                0x89ddff, 0xff7a93,
            ],
        },
        Palette {
            surface: 0xeef2f7,
            panel_solid: 0xffffff,
            content: 0x1b2533,
            accent: 0x2f6fd0,
            link: 0x1f5fbf,
            heading: 0x3b4fb8,
            syntax: [
                0x7847bd, 0x2e7de9, 0x587539, 0x007197, 0xc64343, 0xb15c00, 0x6c7594, 0x387068,
                0x006a83, 0xc64343,
            ],
        },
    ],
    [
        // Moss
        Palette {
            surface: 0x0d110e,
            panel_solid: 0x151b17,
            content: 0xdde5dc,
            accent: 0x6fbf73,
            link: 0x9ad6a0,
            heading: 0xc3d98a,
            syntax: [
                0xe67e80, 0xa7c080, 0xdbbc7f, 0x83c092, 0xe69875, 0xd699b6, 0x859289, 0x7fbbb3,
                0x9da9a0, 0xf85552,
            ],
        },
        Palette {
            surface: 0xf1f4ee,
            panel_solid: 0xffffff,
            content: 0x1f2a20,
            accent: 0x3d8a45,
            link: 0x2e7a3a,
            heading: 0x5d7a1c,
            syntax: [
                0xe0444a, 0x5f7a00, 0x9a7400, 0x2b8a65, 0xc4621a, 0xa3529b, 0x7a8778, 0x2f7fae,
                0x6e7a70, 0xd0353b,
            ],
        },
    ],
    [
        // Ember
        Palette {
            surface: 0x120e0c,
            panel_solid: 0x1c1613,
            content: 0xecdfd6,
            accent: 0xf08a4b,
            link: 0xf6b07c,
            heading: 0xf3a37a,
            syntax: [
                0xfe8019, 0x8ec07c, 0xb8bb26, 0xfabd2f, 0xfb4934, 0xd3869b, 0x928374, 0x83a598,
                0xa89984, 0xff6b5b,
            ],
        },
        Palette {
            surface: 0xf7f2ee,
            panel_solid: 0xffffff,
            content: 0x2d211b,
            accent: 0xc75c1e,
            link: 0xb4501a,
            heading: 0xa8461f,
            syntax: [
                0xaf3a03, 0x427b58, 0x79740e, 0xb57614, 0x9d0006, 0x8f3f71, 0x7c6f64, 0x076678,
                0x665c54, 0xcc241d,
            ],
        },
    ],
    [
        // Orchid
        Palette {
            surface: 0x110d16,
            panel_solid: 0x1a1522,
            content: 0xe6def0,
            accent: 0xb57cf0,
            link: 0xcfa6ff,
            heading: 0xf2a2d6,
            syntax: [
                0xcba6f7, 0x89b4fa, 0xa6e3a1, 0xf9e2af, 0xf38ba8, 0xfab387, 0x7f849c, 0xb4befe,
                0x94e2d5, 0xff8fab,
            ],
        },
        Palette {
            surface: 0xf4f0f8,
            panel_solid: 0xffffff,
            content: 0x261e33,
            accent: 0x8048c8,
            link: 0x6f3bb8,
            heading: 0xa83a85,
            syntax: [
                0x8839ef, 0x1e66f5, 0x2f7d1e, 0xb06a00, 0xd20f39, 0xc94f05, 0x7c7f93, 0x5a65d6,
                0x137a85, 0xd20f39,
            ],
        },
    ],
    [
        // Tidepool
        Palette {
            surface: 0x0a1213,
            panel_solid: 0x111c1e,
            content: 0xd8e8e8,
            accent: 0x3fc1b5,
            link: 0x7fdcd3,
            heading: 0x8ad0e8,
            syntax: [
                0xf28f6b, 0x5fd4e0, 0xa8e6a1, 0xffd580, 0x7fdcd3, 0xc4a7ff, 0x6f8d8f, 0x8ad0e8,
                0x8aa3a4, 0xff7a7a,
            ],
        },
        Palette {
            surface: 0xedf5f5,
            panel_solid: 0xffffff,
            content: 0x162a2b,
            accent: 0x138a80,
            link: 0x0f7a71,
            heading: 0x1d6f8f,
            syntax: [
                0xc2502b, 0x0b7a8a, 0x3c7d2a, 0x8a6500, 0x0f7a71, 0x6b4fc4, 0x6b8285, 0x1d6f8f,
                0x5b7072, 0xc82828,
            ],
        },
    ],
    [
        // Dune
        Palette {
            surface: 0x13110d,
            panel_solid: 0x1d1a14,
            content: 0xebe3d2,
            accent: 0xd9b25c,
            link: 0xe8cc8a,
            heading: 0xe5a86c,
            syntax: [
                0x957fb8, 0x7e9cd8, 0x98bb6c, 0x7aa89f, 0xe46876, 0xd27e99, 0x8a8170, 0xe6c384,
                0xa6a69c, 0xff5d62,
            ],
        },
        Palette {
            surface: 0xf6f1e7,
            panel_solid: 0xfffdf8,
            content: 0x2e2819,
            accent: 0xa57d1e,
            link: 0x8c6914,
            heading: 0x9c5a1c,
            syntax: [
                0x624c83, 0x4d699b, 0x6f894e, 0x597b75, 0xc84053, 0xb35b79, 0x7d7465, 0x8c6914,
                0x716e61, 0xe82424,
            ],
        },
    ],
    [
        // Rosewood
        Palette {
            surface: 0x140e10,
            panel_solid: 0x1e1518,
            content: 0xefdfe3,
            accent: 0xe7798f,
            link: 0xf2a6b4,
            heading: 0xf0b38a,
            syntax: [
                0xeb6f92, 0xebbcba, 0xf6c177, 0x9ccfd8, 0xe7798f, 0xea9a97, 0x7d7891, 0xc4a7e7,
                0x908caa, 0xff5c7a,
            ],
        },
        Palette {
            surface: 0xf8f0f2,
            panel_solid: 0xffffff,
            content: 0x2f1f24,
            accent: 0xc0405c,
            link: 0xa8344f,
            heading: 0xa3502a,
            syntax: [
                0xb4637a, 0xb0605c, 0xa86a0f, 0x286983, 0xc0405c, 0x9a4d2a, 0x797593, 0x7a5f9b,
                0x6e6a86, 0xb4233c,
            ],
        },
    ],
    [
        // Glacier
        Palette {
            surface: 0x0e1114,
            panel_solid: 0x161a1e,
            content: 0xe1e7ec,
            accent: 0x7fb4d9,
            link: 0xa7d0ec,
            heading: 0xb9c8e8,
            syntax: [
                0x81a1c1, 0x88c0d0, 0xa3be8c, 0x8fbcbb, 0xd08770, 0xb48ead, 0x7b88a1, 0xa7c2dc,
                0x7f97b8, 0xbf616a,
            ],
        },
        Palette {
            surface: 0xf2f5f8,
            panel_solid: 0xffffff,
            content: 0x20262d,
            accent: 0x3a7aa8,
            link: 0x2d6a96,
            heading: 0x44598f,
            syntax: [
                0x3a6d9f, 0x2b7a8c, 0x4f7a34, 0x2a7a76, 0xb25a2e, 0x7b4f8a, 0x6b7689, 0x44598f,
                0x5a6a80, 0xb0353f,
            ],
        },
    ],
    [
        // Arcade
        Palette {
            surface: 0x0f0b1a,
            panel_solid: 0x181229,
            content: 0xece6ff,
            accent: 0xff5fd2,
            link: 0x5ff2ff,
            heading: 0xffd35f,
            syntax: [
                0xff5fd2, 0x5ff2ff, 0xffd35f, 0xb38cff, 0xff8f5f, 0xf97e72, 0x7d71a8, 0x72f1b8,
                0x8d86c9, 0xff4f6d,
            ],
        },
        Palette {
            surface: 0xf5f2fc,
            panel_solid: 0xffffff,
            content: 0x231a3a,
            accent: 0xc2189a,
            link: 0x0a7f90,
            heading: 0x9a6608,
            syntax: [
                0xc2189a, 0x0a7f90, 0x8a5a00, 0x6a3fd1, 0xc2410c, 0xc2304a, 0x7a6f99, 0x0f7a55,
                0x6b6390, 0xc81e3a,
            ],
        },
    ],
    [
        // Ink
        Palette {
            surface: 0x000000,
            panel_solid: 0x0a0a0a,
            content: 0xf5f5f5,
            accent: 0x7aa7ff,
            link: 0x9cc0ff,
            heading: 0xffffff,
            syntax: [
                0x7aa7ff, 0xffffff, 0xa8a8a8, 0xd6d6d6, 0x7aa7ff, 0x9cc0ff, 0x7a7a7a, 0xd6d6d6,
                0x8f8f8f, 0xff6b6b,
            ],
        },
        Palette {
            surface: 0xffffff,
            panel_solid: 0xffffff,
            content: 0x000000,
            accent: 0x0050d8,
            link: 0x0040b0,
            heading: 0x000000,
            syntax: [
                0x0050d8, 0x000000, 0x5a5a5a, 0x333333, 0x0050d8, 0x0040b0, 0x767676, 0x2a2a2a,
                0x6a6a6a, 0xc00000,
            ],
        },
    ],
];
