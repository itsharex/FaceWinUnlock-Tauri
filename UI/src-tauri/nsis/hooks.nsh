# 由 Gemini Ai 生成（用不了暂时废弃）
# 1. 首先定义辅助工具函数 (必须放在调用它的函数之前)
Function StrContains
  Exch $R0 ; 查找的目标字符串
  Exch
  Exch $R1 ; 原始长字符串
  Push $R2
  Push $R3
  Push $R4
  Push $R5
  StrLen $R2 $R0
  StrLen $R3 $R1
  StrCpy $R4 0
  loop:
    StrCpy $R5 $R1 $R2 $R4
    StrCmp $R5 $R0 done
    IntOp $R4 $R4 + 1
    IntCmp $R4 $R3 loop loop done
  done:
  StrCpy $R1 $R1 "" $R4
  Pop $R5
  Pop $R4
  Pop $R3
  Pop $R2
  Exch $R1 ; 返回位置
  Exch $R0
  Pop $R0
FunctionEnd

# 2. 定义校验逻辑函数
Function VerifyInstallDir
    # 转换为大写或直接检查特殊路径
    Push $INSTDIR
    Push "Program Files"
    Call StrContains
    Pop $0

    ${If} $0 != ""
        MessageBox MB_ICONSTOP|MB_OK "错误：无法安装在 Program Files 目录下。$\r$\n$\r$\n原因：解锁服务需要 SYSTEM 权限，受系统保护目录（如 Program Files）的 UAC 限制。$\r$\n$\r$\n请选择 C:\facewinunlock-tauri 或其他非系统受限目录。"
        Abort
    ${EndIf}
FunctionEnd

# 3. 使用 Tauri 提供的宏钩子注入初始化逻辑
!macro customInit
    # 强制修改默认安装路径
    StrCpy $INSTDIR "C:\facewinunlock-tauri"
!macroend

# 4. 注入页面离开分支逻辑
# 注意：这行必须在主体脚本展开 Directory Page 之前被执行
!define MUI_PAGE_CUSTOMFUNCTION_LEAVE VerifyInstallDir