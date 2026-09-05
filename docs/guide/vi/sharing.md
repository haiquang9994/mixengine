+++
title = "Cho điện thoại xem site của bạn"
slug = "sharing"
order = 8
summary = "Đưa đúng một site ra mạng nội bộ, quét một mã QR, rồi rút nó về — một site, một cổng, một luật."
translation_of = "en/sharing.md"
source_sha256 = "8978a56454c9ff19e17f9611451ecc43cd6c90091f0564caf10a139ce4156d1a"
+++

# Cho điện thoại xem site của bạn

Mọi thứ MixEngine phục vụ đều chỉ trả lời trên loopback và không ở đâu khác. Thử trên một chiếc điện
thoại thật nghĩa là tạo một ngoại lệ, và ngoại lệ đó là của từng site, có chủ ý, và đảo ngược được.

```bash
mix site share blog.test
```

Lệnh đó in ra một URL mà điện thoại của bạn mở được và một mã QR để chĩa camera vào. Ba việc vừa xảy
ra:

1. Site bắt đầu trả lời trên địa chỉ của máy này trong mạng nội bộ, thay vì chỉ trên loopback. **Chỉ
   site này** — mọi site khác vẫn chỉ trả lời trên loopback.
2. Chứng chỉ được phát hành lại để phủ cả địa chỉ đó, nên ổ khóa sống sót qua chuyến đi.
3. Một hộp thoại quyền quản trị xin một luật tường lửa, cho đúng cổng đó.

## Khi máy có nhiều hơn một mạng

MixEngine **từ chối tự chọn** thay vì đưa site của bạn lên một mạng bạn không có ý — một chiếc
laptop đang ở Wi-Fi văn phòng và đồng thời có VPN là đúng trường hợp mà điều này tồn tại vì nó. Nó
nêu tên các ứng viên, và bạn chọn:

```bash
mix site share blog.test --interface "Wi-Fi"
```

## Một lần chia sẻ tự kết thúc

```bash
mix site share blog.test --for 2h
```

`30s`, `90m`, `2h`, `1d`, hoặc một con số giây trần. Độ dài được tính **từ lúc lần chia sẻ bắt
đầu**, nên xin một độ dài ngắn hơn khoảng thời gian site đã được chia sẻ sẽ bị từ chối chứ không kết
thúc nó ngay tại chỗ.

Không có `--for`, một lần chia sẻ kéo dài cho tới khi bạn kết thúc nó hoặc cho tới khi máy này rời
khỏi mạng mà nó được chia sẻ trên đó. Trường hợp cuối đáng biết: gập laptop lại rồi mở ra ở một nơi
khác là kết thúc lần chia sẻ, vì địa chỉ nó được chia sẻ tại đó không còn là địa chỉ của máy này
nữa.

## Rút nó về

```bash
mix site unshare blog.test
```

Lệnh đó gỡ luật tường lửa, buộc site về lại loopback, và phát hành lại chứng chỉ không kèm địa chỉ
mạng. Một site không đang được chia sẻ thì được để y nguyên, nên chạy nó khi bạn không chắc cũng
không tốn gì.

## Cần biết trước khi dùng

- **Bất kỳ ai trong mạng đó đều với tới được site.** Không có lớp xác thực nào phía trước. Trên mạng
  quán cà phê hay một văn phòng dùng chung, đó là toàn bộ câu chuyện — hãy chia sẻ kèm một độ dài,
  và rút về khi xong.
- **Chứng chỉ vẫn là của MixEngine.** Điện thoại của bạn không tin chứng thực số của MixEngine, nên
  nó sẽ cảnh báo. Chia sẻ là để xem bố cục trên một màn hình thật, không phải để trình diễn ổ khóa.
- **Không có gì thay đổi ở các site khác của bạn.** Luật đó là một cổng, một site, và nó bị hủy bởi
  `unshare`, bởi độ dài hết giờ, hoặc bởi việc rời khỏi mạng.

Mọi thứ hộp thoại xin đều được liệt kê ở
[MixEngine xin quyền để làm gì](./permissions.md).
